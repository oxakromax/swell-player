//! Routes and the navigator that turns them into pages (docs/design.md
//! §2.4).
//!
//! The content is an `AdwViewStack` of four sections (Home, Explore,
//! Search, Collection), each with its own `AdwNavigationView`, so every
//! section keeps its history. Choosing a section switches the stack (and
//! choosing the visible one again pops it to its root); any other page is
//! pushed onto the visible section, popping back to it instead if it is
//! already open there (same tag).
//!
//! Collection's root depends on the layout: in the wide layout it is the
//! chosen sub-view (Tracks, Albums… or a sidebar playlist), replaced when
//! another is chosen; in the narrow one it is the Collection index and
//! sub-views are pushed on top of it. [`Navigator::set_narrow`] re-roots the
//! stack when the layout changes.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use adw::prelude::*;

use super::context::Ctx;
use super::pages;
use crate::i18n::tr;
use crate::tidal::pages::{More, Target};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CollectionView {
    Tracks,
    Albums,
    Artists,
    Playlists,
    Mixes,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Route {
    Home,
    Explore,
    /// Search, optionally with a query to run.
    Search(Option<String>),
    Collection(CollectionView),
    Album(u64),
    Artist(u64),
    Playlist(String),
    Mix(String),
    /// The radio mix of a track.
    TrackRadio(u64),
    /// The radio mix of an artist.
    ArtistRadio(u64),
    /// A v1 editorial page (genre, mood, "explore new music"...).
    Page {
        path: String,
        title: String,
    },
    /// Everything in a section ("View all").
    More {
        title: String,
        more: More,
    },
    AlbumCredits(u64),
    /// Recently played.
    History,
}

/// Names of the sections, in the order of the view switcher.
pub const SECTIONS: [&str; 4] = ["home", "explore", "search", "collection"];

/// Tag of the Collection index page (the narrow layout's Collection root).
pub const COLLECTION_INDEX: &str = "collection-index";

impl Route {
    /// Whether this is a section of the view switcher (switches the
    /// section) rather than a page pushed on top.
    pub fn is_section(&self) -> bool {
        matches!(
            self,
            Self::Home | Self::Explore | Self::Search(_) | Self::Collection(_)
        )
    }

    /// The section this route is the root of, if any.
    pub fn section(&self) -> Option<&'static str> {
        Some(match self {
            Self::Home => "home",
            Self::Explore => "explore",
            Self::Search(_) => "search",
            Self::Collection(_) => "collection",
            _ => return None,
        })
    }

    /// Whether the sidebar opens this route as Collection's root (wide) or
    /// on top of the Collection index (narrow).
    pub fn is_collection_root(&self) -> bool {
        matches!(
            self,
            Self::Collection(_) | Self::History | Self::Playlist(_)
        )
    }

    /// Unique tag of the page, so the same page is not stacked twice.
    pub fn tag(&self) -> String {
        match self {
            Self::Home => "home".into(),
            Self::Explore => "explore".into(),
            Self::Search(_) => "search".into(),
            Self::Collection(view) => format!("collection-{view:?}"),
            Self::Album(id) => format!("album-{id}"),
            Self::Artist(id) => format!("artist-{id}"),
            Self::Playlist(id) => format!("playlist-{id}"),
            Self::Mix(id) => format!("mix-{id}"),
            Self::TrackRadio(id) => format!("track-radio-{id}"),
            Self::ArtistRadio(id) => format!("artist-radio-{id}"),
            Self::Page { path, .. } => format!("page-{path}"),
            Self::More { more, .. } => format!("more-{more:?}"),
            Self::AlbumCredits(id) => format!("credits-{id}"),
            Self::History => "history".into(),
        }
    }

    pub fn from_target(target: &Target) -> Option<Self> {
        Some(match target {
            Target::Album(id) => Self::Album(*id),
            Target::Artist(id) => Self::Artist(*id),
            Target::Playlist(id) => Self::Playlist(id.clone()),
            Target::Mix(id) => Self::Mix(id.clone()),
            Target::Page(path) => Self::Page {
                path: path.clone(),
                title: String::new(),
            },
            Target::Collection(view) => Self::Collection(match view.as_str() {
                "albums" => CollectionView::Albums,
                "artists" => CollectionView::Artists,
                "playlists" => CollectionView::Playlists,
                "mixes" => CollectionView::Mixes,
                _ => CollectionView::Tracks,
            }),
            Target::Track(_) | Target::Url(_) => return None,
        })
    }
}

/// Where the user is, for the sidebar's selection.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Location {
    /// The visible section.
    pub section: String,
    /// Tag of the section's root page (in the narrow layout, Collection's
    /// first page above the index).
    pub root: Option<String>,
    /// Tag of the visible page.
    pub visible: Option<String>,
}

type IndexBuilder = Box<dyn Fn() -> adw::NavigationPage>;
type LocationListener = Rc<dyn Fn(&Location)>;

pub struct Navigator {
    ctx: Weak<Ctx>,
    sections: adw::ViewStack,
    views: [adw::NavigationView; 4],
    narrow: Cell<bool>,
    index_page: RefCell<Option<IndexBuilder>>,
    location: RefCell<Location>,
    on_location: RefCell<Vec<LocationListener>>,
}

impl Navigator {
    pub fn new(ctx: &Rc<Ctx>) -> Rc<Self> {
        let sections = adw::ViewStack::builder().vexpand(true).build();
        let views = SECTIONS.map(|_| adw::NavigationView::new());
        for ((name, view), (title, icon)) in SECTIONS.iter().zip(&views).zip([
            (tr("Home"), "user-home-symbolic"),
            (tr("Explore"), "swell-explore-symbolic"),
            (tr("Search"), "system-search-symbolic"),
            (tr("Collection"), "swell-library-symbolic"),
        ]) {
            sections.add_titled_with_icon(view, Some(name), title, icon);
        }
        let navigator = Rc::new(Self {
            ctx: Rc::downgrade(ctx),
            sections,
            views,
            narrow: Cell::new(false),
            index_page: RefCell::default(),
            location: RefCell::default(),
            on_location: RefCell::default(),
        });
        // The view switcher (narrow) changes the section directly.
        navigator.sections.connect_visible_child_name_notify({
            let navigator = Rc::downgrade(&navigator);
            move |stack| {
                if let Some(navigator) = navigator.upgrade()
                    && let Some(name) = stack.visible_child_name()
                {
                    navigator.ensure_root(&name);
                    navigator.location_changed();
                }
            }
        });
        for view in &navigator.views {
            view.connect_popped(|_, _| crate::mem::trim_soon());
            view.connect_visible_page_notify({
                let navigator = Rc::downgrade(&navigator);
                move |_| {
                    if let Some(navigator) = navigator.upgrade() {
                        navigator.location_changed();
                    }
                }
            });
        }
        ctx.set_navigator(&navigator);
        navigator
    }

    /// The section stack, for the shell to place.
    pub fn sections(&self) -> &adw::ViewStack {
        &self.sections
    }

    /// Builds the Collection index page (narrow layout's Collection root).
    pub fn set_index_page(&self, builder: impl Fn() -> adw::NavigationPage + 'static) {
        self.index_page.replace(Some(Box::new(builder)));
    }

    /// Called whenever the location changes (section or visible page).
    pub fn connect_location_changed(&self, f: impl Fn(&Location) + 'static) {
        self.on_location.borrow_mut().push(Rc::new(f));
    }

    pub fn location(&self) -> Location {
        self.location.borrow().clone()
    }

    /// The navigation view of `section`.
    pub fn view(&self, section: &str) -> &adw::NavigationView {
        let index = SECTIONS.iter().position(|s| *s == section).unwrap_or(0);
        &self.views[index]
    }

    pub fn visible_section(&self) -> String {
        self.sections
            .visible_child_name()
            .map_or_else(|| SECTIONS[0].to_owned(), Into::into)
    }

    /// The navigation view of the visible section.
    pub fn visible_view(&self) -> &adw::NavigationView {
        self.view(&self.visible_section())
    }

    /// Goes back one page in the visible section.
    pub fn back(&self) -> bool {
        self.visible_view().pop()
    }

    /// Shows `section` (building its root page the first time) without
    /// touching its history.
    pub fn show_section(&self, section: &str) {
        self.ensure_root(section);
        self.sections.set_visible_child_name(section);
    }

    /// What choosing a section does: shows it, or pops it to its root if
    /// it is already visible.
    pub fn activate_section(&self, section: &str) {
        if self.visible_section() == section {
            self.pop_to_root(section);
        } else {
            self.show_section(section);
        }
    }

    pub fn navigate(&self, route: Route) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        // Recently Played belongs to Collection, wherever it is opened from.
        if let Route::Collection(_) | Route::History = route {
            return self.open_collection_root(route);
        }
        if let Some(section) = route.section() {
            if let Route::Search(Some(_)) = route {
                // A new query replaces the search page.
                self.view(section).replace(&[build(&ctx, &route)]);
                crate::mem::trim_soon();
                self.sections.set_visible_child_name(section);
                return;
            }
            return self.activate_section(section);
        }
        let view = self.visible_view().clone();
        self.ensure_root(&self.visible_section());
        let tag = route.tag();
        if view.find_page(&tag).is_some() {
            view.pop_to_tag(&tag);
        } else {
            view.push(&build(&ctx, &route));
        }
    }

    /// Opens a Collection sub-view, Recently played or a playlist the way
    /// the sidebar and the Collection index do: as Collection's root in the
    /// wide layout, on top of the index in the narrow one. Other routes
    /// fall back to [`Navigator::navigate`].
    pub fn open_collection_root(&self, route: Route) {
        if !route.is_collection_root() {
            return self.navigate(route);
        }
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        let view = self.view("collection");
        let tag = route.tag();
        let stack = tags(view);
        if self.narrow.get() {
            match stack.first().map(String::as_str) {
                Some(COLLECTION_INDEX) => {
                    if stack.get(1) == Some(&tag) {
                        view.pop_to_tag(&tag);
                    } else {
                        let index = page_at(view, 0);
                        if let Some(index) = index {
                            view.pop_to_page(&index);
                        }
                        view.push(&build(&ctx, &route));
                    }
                }
                _ => {
                    let index = self.build_index();
                    view.replace(&[index, build(&ctx, &route)]);
                }
            }
        } else if stack.first() == Some(&tag) {
            self.pop_to_root("collection");
        } else {
            view.replace(&[build(&ctx, &route)]);
        }
        crate::mem::trim_soon();
        self.sections.set_visible_child_name("collection");
    }

    /// Switches between the wide and narrow layouts, re-rooting Collection
    /// (`[sub-view, …]` ⇄ `[index, sub-view, …]`).
    pub fn set_narrow(&self, narrow: bool) {
        if self.narrow.replace(narrow) == narrow {
            return;
        }
        let view = self.view("collection");
        let pages: Vec<adw::NavigationPage> = (0..view.navigation_stack().n_items())
            .filter_map(|i| page_at(view, i))
            .collect();
        let Some(first) = pages.first() else {
            return;
        };
        let has_index = first.tag().as_deref() == Some(COLLECTION_INDEX);
        if narrow && !has_index {
            let mut stack = vec![self.build_index()];
            stack.extend(pages);
            view.replace(&stack);
        } else if !narrow && has_index {
            let rest = &pages[1..];
            if rest.is_empty() {
                if let Some(ctx) = self.ctx.upgrade() {
                    let tracks = Route::Collection(CollectionView::Tracks);
                    view.replace(&[build(&ctx, &tracks)]);
                }
            } else {
                view.replace(rest);
            }
        }
        self.location_changed();
    }

    pub fn pop_to_root(&self, section: &str) {
        let view = self.view(section);
        if let Some(root) = page_at(view, 0) {
            view.pop_to_page(&root);
        }
    }

    /// Builds the root page of `section` if it has none yet (sections are
    /// built lazily, the first time they are shown).
    fn ensure_root(&self, section: &str) {
        let view = self.view(section);
        if view.navigation_stack().n_items() > 0 {
            return;
        }
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        let page = match section {
            "explore" => build(&ctx, &Route::Explore),
            "search" => build(&ctx, &Route::Search(None)),
            "collection" if self.narrow.get() => self.build_index(),
            "collection" => build(&ctx, &Route::Collection(CollectionView::Tracks)),
            _ => build(&ctx, &Route::Home),
        };
        view.replace(&[page]);
    }

    fn build_index(&self) -> adw::NavigationPage {
        let page = match self.index_page.borrow().as_ref() {
            Some(builder) => builder(),
            None => adw::NavigationPage::builder()
                .title(tr("Collection"))
                .build(),
        };
        page.set_tag(Some(COLLECTION_INDEX));
        page
    }

    fn location_changed(&self) {
        let section = self.visible_section();
        let view = self.view(&section);
        let stack = tags(view);
        let root = match stack.first().map(String::as_str) {
            Some(COLLECTION_INDEX) => stack.get(1).cloned(),
            _ => stack.first().cloned(),
        };
        let location = Location {
            section,
            root,
            visible: view.visible_page_tag().map(Into::into),
        };
        if *self.location.borrow() == location {
            return;
        }
        self.location.replace(location.clone());
        let listeners: Vec<_> = self.on_location.borrow().clone();
        for listener in listeners {
            listener(&location);
        }
    }
}

/// Gives memory freed by dropped pages back to the system, on idle.

/// Builds the page of `route`, tagged.
fn build(ctx: &Rc<Ctx>, route: &Route) -> adw::NavigationPage {
    let page = pages::build(ctx, route);
    page.set_tag(Some(&route.tag()));
    page
}

fn page_at(view: &adw::NavigationView, index: u32) -> Option<adw::NavigationPage> {
    view.navigation_stack()
        .item(index)
        .and_downcast::<adw::NavigationPage>()
}

/// Tags of the pages in `view`'s stack, bottom first.
fn tags(view: &adw::NavigationView) -> Vec<String> {
    (0..view.navigation_stack().n_items())
        .filter_map(|i| page_at(view, i))
        .map(|page| page.tag().map(Into::into).unwrap_or_default())
        .collect()
}
