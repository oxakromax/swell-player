//! Home (docs/design.md §3.2): the personalised v2 feed. Shortcuts first,
//! then the feed's carousels and short track lists in its order. The first
//! batch of modules comes from the cache (fresh for ten minutes, then
//! revalidated); further batches are fetched by cursor when the end of the
//! page is less than a screen away. F5 fetches the feed again.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;

use super::super::context::Ctx;
use super::super::nav::Route;
use super::super::widgets::status::{describe_error, empty_status};
use super::home_sections::{self, Scaffold, Shape};
use crate::i18n::tr;
use crate::runtime;
use crate::tidal::pages::{self, Feed};
use crate::tidal::{self, Base};

const FRESH_FOR: Duration = Duration::from_secs(10 * 60);
const FEED_PATH: &str = "home/feed/static?platform=WEB";

struct HomePage {
    ctx: Weak<Ctx>,
    view: Scaffold,
    /// Where the next batch of modules starts, while there is one.
    cursor: RefCell<Option<String>>,
    loading_more: Cell<bool>,
    /// A batch failed: wait for "Retry" instead of asking again on scroll.
    more_failed: Cell<bool>,
    task: RefCell<Option<glib::JoinHandle<()>>>,
    more_task: RefCell<Option<glib::JoinHandle<()>>>,
}

pub fn page(ctx: &Rc<Ctx>) -> adw::NavigationPage {
    let (view, page) = Scaffold::new(ctx, tr("Home"), true, Shape::Feed);
    view.heading.set_label(greeting());
    let state = Rc::new(HomePage {
        ctx: Rc::downgrade(ctx),
        view,
        cursor: RefCell::default(),
        loading_more: Cell::new(false),
        more_failed: Cell::new(false),
        task: RefCell::default(),
        more_task: RefCell::default(),
    });

    let weak = Rc::downgrade(&state);
    state.view.connect_near_end(move || {
        if let Some(state) = weak.upgrade() {
            state.load_more();
        }
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
        if let Some(state) = owned.take() {
            state.abort();
        }
    });
    page
}

/// "Good Morning", "Good Afternoon" or "Good Evening".
fn greeting() -> &'static str {
    let hour = glib::DateTime::now_local().map_or(12, |now| now.hour());
    match hour {
        5..=11 => tr("Good Morning"),
        12..=17 => tr("Good Afternoon"),
        _ => tr("Good Evening"),
    }
}

impl HomePage {
    fn abort(&self) {
        for task in [self.task.take(), self.more_task.take()]
            .into_iter()
            .flatten()
        {
            task.abort();
        }
        self.loading_more.set(false);
    }

    /// Loads the feed's first batch: the cached one at once, then the
    /// fresh one if the cache is older than `fresh_for`.
    fn load(self: &Rc<Self>, fresh_for: Duration) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        self.abort();
        if !self.view.stack.is_showing_content() {
            self.view.stack.show_loading();
        }
        let mut answers =
            ctx.api
                .load_cached(Base::V2, FEED_PATH.into(), fresh_for, pages::parse_feed);
        let weak = Rc::downgrade(self);
        let task = glib::spawn_future_local(async move {
            while let Some(answer) = answers.recv().await {
                let Some(state) = weak.upgrade() else {
                    return;
                };
                match answer {
                    Ok(feed) => state.show(feed),
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
            ctx.report(tr("Couldn't load Home"), err);
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

    fn show(self: &Rc<Self>, feed: Feed) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        if let Some(task) = self.more_task.take() {
            task.abort();
        }
        self.loading_more.set(false);
        self.more_failed.set(false);
        self.view.heading.set_label(greeting());
        self.view.clear();
        let shown = self.append(&ctx, &feed);
        if !shown {
            let status = empty_status(
                "user-home-symbolic",
                tr("Nothing Here Yet"),
                tr("Play some music and TIDAL will suggest more here."),
            );
            let button = gtk::Button::builder()
                .label(tr("Explore"))
                .halign(gtk::Align::Center)
                .css_classes(["pill", "suggested-action"])
                .build();
            let weak = Rc::downgrade(&ctx);
            button.connect_clicked(move |_| {
                if let Some(ctx) = weak.upgrade() {
                    ctx.navigate(Route::Explore);
                }
            });
            status.set_child(Some(&button));
            self.view.sections.append(&status);
        }
        self.cursor.replace(feed.cursor);
        self.view.stack.show_content();
    }

    /// Appends the modules of `feed`; whether any was shown.
    fn append(&self, ctx: &Rc<Ctx>, feed: &Feed) -> bool {
        let mut shown = false;
        for section in &feed.sections {
            if let Some(widget) = home_sections::section_widget(ctx, section) {
                self.view.sections.append(&widget);
                shown = true;
            }
        }
        shown
    }

    /// Fetches the next batch of modules, if there is one.
    fn load_more(self: &Rc<Self>) {
        if self.loading_more.get()
            || self.more_failed.get()
            || !self.view.stack.is_showing_content()
        {
            return;
        }
        let Some(cursor) = self.cursor.borrow().clone() else {
            return;
        };
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        self.loading_more.set(true);
        self.view.set_footer(Some(&home_sections::loading_more()));
        let api = ctx.api.clone();
        let weak = Rc::downgrade(self);
        let task = glib::spawn_future_local(async move {
            let answer = runtime::spawn(async move { api.home(Some(&cursor)).await }).await;
            let Some(state) = weak.upgrade() else {
                return;
            };
            state.loading_more.set(false);
            state.more_task.take();
            let Some(ctx) = state.ctx.upgrade() else {
                return;
            };
            match answer {
                Ok(feed) => {
                    state.view.set_footer(None);
                    state.append(&ctx, &feed);
                    state.cursor.replace(feed.cursor);
                }
                Err(err) => {
                    if tidal::is_auth_error(&err) {
                        ctx.report(tr("Couldn't load more of Home"), &err);
                    }
                    log::warn!("home feed: {err:#}");
                    state.more_failed.set(true);
                    let weak = Rc::downgrade(&state);
                    let message = tr("Couldn't load more. {reason}")
                        .replace("{reason}", &describe_error(&err));
                    state.view.set_footer(Some(&home_sections::paging_error(
                        &message,
                        move || {
                            if let Some(state) = weak.upgrade() {
                                state.more_failed.set(false);
                                state.load_more();
                            }
                        },
                    )));
                }
            }
        });
        self.more_task.replace(Some(task));
    }
}
