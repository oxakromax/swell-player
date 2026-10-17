//! The favourite toggle (docs/design.md §4.6): a heart for tracks, albums,
//! playlists and mixes, a "Follow" / "Following" pill for artists.
//!
//! Optimistic: the toggle flips at once ([`Library::set_favorite`] applies
//! the change before asking TIDAL) and flips back with a toast if TIDAL
//! refuses.
//!
//! [`Library::set_favorite`]: crate::library::Library::set_favorite

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gtk::glib;
use gtk::prelude::*;

use crate::library::{Change, Item};
use crate::ui::context::Ctx;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Style {
    Heart,
    Follow,
}

/// Owned by the button's `toggled` handler, so it lives exactly as long as
/// the button; it holds the button weakly.
struct Inner {
    ctx: Weak<Ctx>,
    button: glib::WeakRef<gtk::ToggleButton>,
    content: Option<adw::ButtonContent>,
    style: Style,
    item: RefCell<Option<Item>>,
    /// Set while the state is changed from code, so `toggled` is ignored.
    updating: Cell<bool>,
}

/// Cheap to clone; clones share the button.
#[derive(Clone)]
pub struct HeartButton {
    button: gtk::ToggleButton,
    inner: Rc<Inner>,
}

impl HeartButton {
    /// A heart that follows the library's changes on its own (headers,
    /// cards, the player).
    pub fn new(ctx: &Rc<Ctx>) -> Self {
        let this = Self::build(ctx, Style::Heart);
        this.listen(ctx);
        this
    }

    /// The artist "Follow" toggle (`pill`, label changes with the state).
    pub fn follow(ctx: &Rc<Ctx>) -> Self {
        let this = Self::build(ctx, Style::Follow);
        this.listen(ctx);
        this
    }

    /// A heart that does not listen to the library itself: the owner (one
    /// list for all its rows) calls [`HeartButton::sync`] when needed.
    pub fn unlistened(ctx: &Rc<Ctx>) -> Self {
        Self::build(ctx, Style::Heart)
    }

    fn build(ctx: &Rc<Ctx>, style: Style) -> Self {
        let button = gtk::ToggleButton::builder()
            .valign(gtk::Align::Center)
            .css_classes(["heart"])
            .build();
        let content = match style {
            Style::Heart => {
                button.add_css_class("circular");
                button.add_css_class("flat");
                button.update_property(&[gtk::accessible::Property::Label("Favourite")]);
                None
            }
            Style::Follow => {
                button.add_css_class("pill");
                let content = adw::ButtonContent::new();
                button.set_child(Some(&content));
                Some(content)
            }
        };
        let inner = Rc::new(Inner {
            ctx: Rc::downgrade(ctx),
            button: button.downgrade(),
            content,
            style,
            item: RefCell::default(),
            updating: Cell::new(false),
        });
        let owned = inner.clone();
        button.connect_toggled(move |_| owned.clone().toggled());
        let this = Self { button, inner };
        this.set_item(None);
        this
    }

    fn listen(&self, ctx: &Rc<Ctx>) {
        let weak = Rc::downgrade(&self.inner);
        let id = ctx.library.connect_changed(move |change| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            match change {
                Change::Favorite(item, _) if inner.item.borrow().as_ref() == Some(item) => {
                    inner.sync()
                }
                Change::Loaded => inner.sync(),
                _ => {}
            }
        });
        let library = Rc::downgrade(&ctx.library);
        self.button.connect_destroy(move |_| {
            if let Some(library) = library.upgrade() {
                library.disconnect(id);
            }
        });
    }

    pub fn widget(&self) -> &gtk::ToggleButton {
        &self.button
    }

    /// What the button is the favourite toggle of (`None`: insensitive).
    pub fn set_item(&self, item: Option<Item>) {
        self.button.set_sensitive(item.is_some());
        self.inner.item.replace(item);
        self.inner.sync();
    }

    pub fn item(&self) -> Option<Item> {
        self.inner.item.borrow().clone()
    }

    /// Re-reads the state from the library.
    pub fn sync(&self) {
        self.inner.sync();
    }
}

impl Inner {
    fn sync(&self) {
        let Some(button) = self.button.upgrade() else {
            return;
        };
        let favorite = match (self.ctx.upgrade(), self.item.borrow().as_ref()) {
            (Some(ctx), Some(item)) => ctx.library.is_favorite(item),
            _ => false,
        };
        if button.is_active() != favorite {
            self.updating.set(true);
            button.set_active(favorite);
            self.updating.set(false);
        }
        self.show(&button, favorite);
    }

    fn show(&self, button: &gtk::ToggleButton, favorite: bool) {
        match self.style {
            Style::Heart => {
                let icon = if favorite {
                    super::icon_name("swell-heart-filled-symbolic", "starred-symbolic")
                } else {
                    super::icon_name("swell-heart-symbolic", "non-starred-symbolic")
                };
                button.set_icon_name(icon);
                button.set_tooltip_text(Some(if favorite {
                    "Remove from Favourites"
                } else {
                    "Add to Favourites"
                }));
            }
            Style::Follow => {
                if let Some(content) = &self.content {
                    if favorite {
                        content.set_icon_name("object-select-symbolic");
                        content.set_label("Following");
                    } else {
                        content.set_icon_name("list-add-symbolic");
                        content.set_label("Follow");
                    }
                }
                button.set_tooltip_text(Some(if favorite {
                    "Unfollow"
                } else {
                    "Follow this artist"
                }));
            }
        }
    }

    fn toggled(self: Rc<Self>) {
        if self.updating.get() {
            return;
        }
        let (Some(ctx), Some(button), Some(item)) = (
            self.ctx.upgrade(),
            self.button.upgrade(),
            self.item.borrow().clone(),
        ) else {
            return;
        };
        let favorite = button.is_active();
        self.show(&button, favorite);
        let weak = Rc::downgrade(&self);
        let report = Rc::downgrade(&ctx);
        ctx.library.set_favorite(item, favorite, move |result| {
            if let Err(err) = result {
                if let Some(ctx) = report.upgrade() {
                    ctx.report("Couldn't update favourites", &err);
                }
                if let Some(inner) = weak.upgrade() {
                    inner.sync();
                }
            }
        });
    }
}
