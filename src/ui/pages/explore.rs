//! Explore and the editorial pages it leads to (docs/design.md §3.3).
//!
//! Explore is TIDAL's v1 `pages/explore`: promotions, then genres, moods
//! and decades as chips that open their own page. Those pages (and any
//! other v1 page, such as `pages/explore_new_music`) are rendered by the
//! same builder with the same module grammar as Home. Pages are cached for
//! an hour and revalidated in the background; F5 fetches them again.

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;

use super::super::context::Ctx;
use super::super::widgets::status::empty_status;
use super::home_sections::{self, Scaffold, Shape};
use crate::tidal::pages::{self, Page};
use crate::tidal::{self, Base};

const FRESH_FOR: Duration = Duration::from_secs(60 * 60);
const EXPLORE_PATH: &str = "pages/explore";

struct EditorialPage {
    ctx: Weak<Ctx>,
    path: String,
    /// Whether the page title comes from TIDAL (the route had none).
    titled_by_page: bool,
    view: Scaffold,
    task: RefCell<Option<glib::JoinHandle<()>>>,
}

/// Explore (`None`), or the editorial page `path` with its `title` (empty:
/// the page's own title).
pub fn page(ctx: &Rc<Ctx>, page: Option<(String, String)>) -> adw::NavigationPage {
    let root = page.is_none();
    let (path, title) = page.unwrap_or_else(|| (EXPLORE_PATH.into(), "Explore".into()));
    let shape = if root { Shape::Chips } else { Shape::Carousels };
    let view = Scaffold::new(ctx, &title, root, shape);
    if title.is_empty() {
        view.page.set_title("Explore");
    }
    let page = view.page.clone();
    let state = Rc::new(EditorialPage {
        ctx: Rc::downgrade(ctx),
        path,
        titled_by_page: title.is_empty(),
        view,
        task: RefCell::default(),
    });

    let weak = Rc::downgrade(&state);
    state.view.connect_reload(move || {
        if let Some(state) = weak.upgrade() {
            state.load(Duration::ZERO);
        }
    });

    state.load(FRESH_FOR);
    let owned = RefCell::new(Some(state));
    page.connect_destroy(move |_| {
        if let Some(state) = owned.take()
            && let Some(task) = state.task.take()
        {
            task.abort();
        }
    });
    page
}

impl EditorialPage {
    fn load(self: &Rc<Self>, fresh_for: Duration) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        if let Some(task) = self.task.take() {
            task.abort();
        }
        if !self.view.stack.is_showing_content() {
            self.view.stack.show_loading();
        }
        let mut answers =
            ctx.api
                .load_cached(Base::V1, self.path.clone(), fresh_for, pages::parse_v1);
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
            ctx.report("Couldn't load the page", err);
        }
        if self.view.stack.is_showing_content() {
            return;
        }
        let weak = Rc::downgrade(self);
        self.view.stack.show_error(err, move || {
            if let Some(state) = weak.upgrade() {
                state.load(FRESH_FOR);
            }
        });
    }

    fn show(self: &Rc<Self>, page: Page) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        if self.titled_by_page && !page.title.is_empty() {
            self.view.set_title(&page.title);
        }
        self.view.clear();
        let mut shown = false;
        for section in &page.sections {
            if let Some(widget) = home_sections::section_widget(&ctx, section) {
                self.view.sections.append(&widget);
                shown = true;
            }
        }
        if !shown {
            self.view.sections.append(&empty_status(
                "folder-music-symbolic",
                "Nothing Here Yet",
                "TIDAL has nothing on this page in your country.",
            ));
        }
        self.view.stack.show_content();
    }
}
