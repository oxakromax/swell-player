//! Routes and the navigator that turns them into pages.
//!
//! Top-level sections (Home, Explore, Search, Collection views) replace the
//! content's root page; everything else is pushed onto the navigation stack.
//! Pushing a page that is already open (same tag) pops back to it instead of
//! stacking a duplicate.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use adw::prelude::*;

use super::context::Ctx;
use super::pages;
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
    Page { path: String, title: String },
    /// Everything in a section ("View all").
    More { title: String, more: More },
    AlbumCredits(u64),
}

impl Route {
    /// Whether this is a sidebar section (replaces the root) rather than a
    /// page pushed on top.
    pub fn is_section(&self) -> bool {
        matches!(
            self,
            Self::Home | Self::Explore | Self::Search(_) | Self::Collection(_)
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

pub struct Navigator {
    ctx: Weak<Ctx>,
    view: adw::NavigationView,
    /// The current root section, so re-selecting it only pops to it.
    root: RefCell<Option<Route>>,
    on_section: RefCell<Option<Box<dyn Fn(&Route)>>>,
}

impl Navigator {
    pub fn new(ctx: &Rc<Ctx>, view: adw::NavigationView) -> Rc<Self> {
        let navigator = Rc::new(Self {
            ctx: Rc::downgrade(ctx),
            view,
            root: RefCell::default(),
            on_section: RefCell::default(),
        });
        ctx.set_navigator(&navigator);
        navigator
    }

    pub fn view(&self) -> &adw::NavigationView {
        &self.view
    }

    /// Called when the root section changes (to sync the sidebar).
    pub fn connect_section_changed(&self, f: impl Fn(&Route) + 'static) {
        self.on_section.replace(Some(Box::new(f)));
    }

    pub fn current_section(&self) -> Option<Route> {
        self.root.borrow().clone()
    }

    pub fn navigate(&self, route: Route) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        if route.is_section() {
            let same_section = self
                .root
                .borrow()
                .as_ref()
                .is_some_and(|root| root.tag() == route.tag());
            if same_section {
                // Pop to the section; a search with a query still runs it.
                if let Some(root) = self.view.navigation_stack().item(0).and_downcast::<adw::NavigationPage>() {
                    self.view.pop_to_page(&root);
                }
                if let Route::Search(Some(_)) = route {
                    let page = pages::build(&ctx, &route);
                    self.view.replace(&[page]);
                }
                return;
            }
            let page = pages::build(&ctx, &route);
            self.view.replace(&[page]);
            self.root.replace(Some(route.clone()));
            if let Some(f) = self.on_section.borrow().as_ref() {
                f(&route);
            }
            return;
        }
        let tag = route.tag();
        if self.view.find_page(&tag).is_some() {
            self.view.pop_to_tag(&tag);
            return;
        }
        let page = pages::build(&ctx, &route);
        page.set_tag(Some(&tag));
        self.view.push(&page);
    }
}
