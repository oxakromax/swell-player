//! One module per page. Each exposes `page(ctx, ...) -> adw::NavigationPage`
//! and loads its own data without blocking the main thread.

mod album;
mod artist;
mod collection;
mod explore;
mod history;
mod home;
mod mix;
mod more;
mod playlist;
mod search;
mod search_data;
mod search_views;

use std::rc::Rc;

use super::context::Ctx;
use super::nav::Route;

pub fn build(ctx: &Rc<Ctx>, route: &Route) -> adw::NavigationPage {
    match route {
        Route::Home => home::page(ctx),
        Route::Explore => explore::page(ctx, None),
        Route::Page { path, title } => explore::page(ctx, Some((path.clone(), title.clone()))),
        Route::Search(query) => search::page(ctx, query.clone()),
        Route::Collection(view) => collection::page(ctx, *view),
        Route::Album(id) => album::page(ctx, *id),
        Route::AlbumCredits(id) => album::credits_page(ctx, *id),
        Route::Artist(id) => artist::page(ctx, *id),
        Route::Playlist(id) => playlist::page(ctx, id.clone()),
        Route::Mix(id) => mix::page(ctx, mix::Kind::Mix(id.clone())),
        Route::TrackRadio(id) => mix::page(ctx, mix::Kind::TrackRadio(*id)),
        Route::ArtistRadio(id) => mix::page(ctx, mix::Kind::ArtistRadio(*id)),
        Route::More { title, more } => more::page(ctx, title.clone(), more.clone()),
        Route::History => history::page(ctx),
    }
}

/// A page that is not built yet.
pub(super) fn placeholder(title: &str) -> adw::NavigationPage {
    let status = adw::StatusPage::builder()
        .icon_name("applications-engineering-symbolic")
        .title(title)
        .description("Coming soon")
        .build();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&status));
    adw::NavigationPage::builder()
        .title(title)
        .child(&toolbar)
        .build()
}
