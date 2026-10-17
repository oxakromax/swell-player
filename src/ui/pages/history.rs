//! Recently played (docs/design.md §3.12).
//!
//! One virtualised list: what played in this session (from the queue's
//! history, newest first, refreshed as tracks change) as rows grouped by
//! day ("Today", "Yesterday", "This Week", "Earlier") with when each one
//! played ("5 min ago"), then, as the list's footer, what TIDAL remembers
//! across devices (the home feed's "Recently played" module,
//! `home/pages/CONTINUE_LISTEN_TO/view-all`) as cards. The cards come from
//! the response cache first.
//!
//! The queue does not keep times: [`record_plays`] notes when each track
//! starts playing, from the first page built on. Tracks without a time
//! (restored from the last run) are "Earlier".

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;

use super::super::context::Ctx;
use super::super::format;
use super::super::menus::ItemMenu;
use super::super::sidebar;
use super::super::widgets::carousel::title_row;
use super::super::widgets::media_card::MediaCard;
use super::super::widgets::section;
use super::super::widgets::skeleton;
use super::super::widgets::status::{describe_error, empty_status};
use super::super::widgets::track_list::{Rows, TrackList, Variant, compact_bin};
use crate::i18n::tr;
use crate::model::TrackInfo;
use crate::playback::{Event, PlayState, Source};
use crate::tidal::pages::{self, Section};
use crate::tidal::{self, Base};

/// The `viewAll` of the home feed's CONTINUE_LISTEN_TO module.
const PATH: &str = "home/pages/CONTINUE_LISTEN_TO/view-all?platform=WEB";
const FRESH_FOR: Duration = Duration::from_secs(2 * 60);

thread_local! {
    /// Track ids and when they started playing (Unix seconds), oldest
    /// first, for the session of [`RECORDING`].
    static PLAYED: RefCell<Vec<(u64, i64)>> = RefCell::default();
    /// The session (`Ctx`) being recorded.
    static RECORDING: RefCell<Weak<Ctx>> = RefCell::default();
}

/// Notes when tracks start playing in `ctx`'s session (once per session;
/// a new session starts a new record).
pub(super) fn record_plays(ctx: &Rc<Ctx>) {
    let current = RECORDING.with(|r| r.borrow().upgrade());
    if current.is_some_and(|current| Rc::ptr_eq(&current, ctx)) {
        return;
    }
    RECORDING.with(|r| r.replace(Rc::downgrade(ctx)));
    PLAYED.with(|p| p.borrow_mut().clear());
    // A track counts as played once it actually plays (not when a paused
    // session is restored).
    let now = || glib::DateTime::now_utc().map_or(0, |now| now.to_unix());
    let current = ctx.playback.current().map(|t| t.id);
    let pending = Cell::new(None::<u64>);
    match (current, ctx.playback.state()) {
        (Some(id), PlayState::Playing) => PLAYED.with(|p| p.borrow_mut().push((id, now()))),
        (id, _) => pending.set(id),
    }
    ctx.playback.subscribe(move |event| match event {
        Event::Track(track) => pending.set(track.as_ref().map(|t| t.id)),
        Event::State(PlayState::Playing) => {
            if let Some(id) = pending.take() {
                PLAYED.with(|p| p.borrow_mut().push((id, now())));
            }
        }
        _ => {}
    });
}

/// When each of `tracks` (newest first) started playing, as far as known:
/// matched newest first against the record.
fn play_times(tracks: &[TrackInfo]) -> Vec<Option<i64>> {
    PLAYED.with(|played| {
        let played = played.borrow();
        let mut rest = played.iter().rev();
        tracks
            .iter()
            .map(|track| {
                let found = rest.clone().position(|(id, _)| *id == track.id)?;
                rest.nth(found).map(|(_, time)| *time)
            })
            .collect()
    })
}

/// The day groups of Recently Played.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Day {
    Today,
    Yesterday,
    ThisWeek,
    Earlier,
}

impl Day {
    fn title(self) -> &'static str {
        match self {
            Self::Today => tr("Today"),
            Self::Yesterday => tr("Yesterday"),
            Self::ThisWeek => tr("This Week"),
            Self::Earlier => tr("Earlier"),
        }
    }
}

/// The group of something played at `time` (Unix seconds) seen at `now`
/// (local time), and how long ago it was: "Just now", "5 min ago",
/// "3 h ago", then the time of day ("14:05") or the weekday and time
/// ("Mon 14:05"), or the date ("Oct 3").
fn when(now: &glib::DateTime, time: Option<i64>) -> (Day, Option<String>) {
    let Some(played) = time.and_then(|time| glib::DateTime::from_unix_local(time).ok()) else {
        return (Day::Earlier, None);
    };
    let days = days_between(&played, now);
    let seconds = now.to_unix() - played.to_unix();
    let day = match days {
        0 => Day::Today,
        1 => Day::Yesterday,
        2..=6 => Day::ThisWeek,
        _ => Day::Earlier,
    };
    let note = if seconds < 60 {
        tr("Just now").to_owned()
    } else if seconds < 3600 {
        tr("{n} min ago").replace("{n}", &(seconds / 60).to_string())
    } else if days == 0 {
        tr("{n} h ago").replace("{n}", &(seconds / 3600).to_string())
    } else {
        let format = match day {
            Day::Yesterday => "%H:%M",
            Day::ThisWeek => "%a %H:%M",
            // TRANSLATORS: a day of an earlier week (strftime format:
            // %b is the month's short name).
            _ => tr("%b %-d"),
        };
        format::date(&played, format)
    };
    (day, Some(note))
}

/// Calendar days from `earlier` to `later` (local dates).
fn days_between(earlier: &glib::DateTime, later: &glib::DateTime) -> i64 {
    let date = |time: &glib::DateTime| {
        glib::DateTime::new(
            &time.timezone(),
            time.year(),
            time.month(),
            time.day_of_month(),
            12,
            0,
            0.0,
        )
        .map_or(0, |noon| noon.to_unix())
    };
    ((date(later) - date(earlier)) as f64 / 86_400.0).round() as i64
}

struct HistoryPage {
    ctx: Weak<Ctx>,
    list: TrackList,
    stack: gtk::Stack,
    /// "Tracks you play appear here" while the session has none.
    session_hint: gtk::Label,
    /// The TIDAL part: skeleton, cards or error.
    remote: gtk::Stack,
    /// Whether TIDAL has anything (for the empty state of the whole page).
    remote_items: std::cell::Cell<bool>,
    task: RefCell<Option<glib::JoinHandle<()>>>,
}

pub fn page(ctx: &Rc<Ctx>) -> adw::NavigationPage {
    let list = TrackList::new(ctx, Variant::Cover);
    list.set_source(Source {
        title: tr("Recently Played").into(),
        target: None,
    });

    let header = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(18)
        .margin_bottom(6)
        .build();
    section::align_to_edge(&header, 0);
    header.append(&title_row(tr("This Session"), None));
    let session_hint = gtk::Label::builder()
        .label(tr("Tracks you play in Swell appear here."))
        .xalign(0.0)
        .wrap(true)
        .css_classes(["dimmed"])
        .build();
    header.append(&session_hint);
    list.set_header(Some(&header));

    let remote = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .vhomogeneous(false)
        .build();
    let ghosts = adw::WrapBox::builder()
        .child_spacing(12)
        .line_spacing(12)
        .build();
    for _ in 0..6 {
        ghosts.append(&skeleton::card(160));
    }
    // Ghosts and hints start where card covers do.
    ghosts.set_margin_start(section::CARD_INSET);
    remote.add_named(&ghosts, Some("loading"));
    // Cards line up with the content edge (they have padding of their own).
    section::align_to_edge(&remote, section::CARD_INSET);
    let footer = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(30)
        .margin_bottom(36)
        .build();
    let footer_title = title_row(
        tr("On TIDAL"),
        Some(tr("What you played lately, on any device")),
    );
    footer_title.add_css_class("page-content");
    footer.append(&footer_title);
    footer.append(&remote);
    list.set_footer(Some(&footer));

    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .build();
    stack.add_named(list.widget(), Some("content"));
    stack.add_named(
        &empty_status(
            "document-open-recent-symbolic",
            tr("Nothing Played Yet"),
            tr("Music you play appears here."),
        ),
        Some("empty"),
    );

    let title = adw::WindowTitle::new(tr("Recently Played"), "");
    let header_bar = adw::HeaderBar::builder().title_widget(&title).build();
    header_bar.pack_start(&sidebar::show_sidebar_button(&ctx.window));
    header_bar.pack_end(&sidebar::narrow_main_menu_button(&ctx.window));
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header_bar);
    let page = adw::NavigationPage::builder()
        .title(tr("Recently Played"))
        .child(&toolbar)
        .build();

    let state = Rc::new(HistoryPage {
        ctx: Rc::downgrade(ctx),
        list: list.clone(),
        stack,
        session_hint,
        remote,
        remote_items: std::cell::Cell::new(false),
        task: RefCell::default(),
    });
    let weak = Rc::downgrade(&state);
    let bin = compact_bin(&state.stack, move |compact| {
        if let Some(state) = weak.upgrade() {
            state.list.set_compact(compact);
        }
    });
    toolbar.set_content(Some(&bin));

    let weak = Rc::downgrade(&state);
    let subscription = ctx.playback.subscribe(move |event| {
        if let (Event::Track(_) | Event::State(PlayState::Playing), Some(state)) =
            (event, weak.upgrade())
        {
            state.show_session();
        }
    });

    state.show_session();
    // "5 min ago" moves on: refresh while the page lives.
    let weak = Rc::downgrade(&state);
    glib::timeout_add_seconds_local(60, move || match weak.upgrade() {
        Some(state) => {
            if state.list.widget().is_mapped() {
                state.show_session();
            }
            glib::ControlFlow::Continue
        }
        None => glib::ControlFlow::Break,
    });
    state.load_remote();
    let weak_ctx = Rc::downgrade(ctx);
    let owned = RefCell::new(Some(state));
    page.connect_destroy(move |_| {
        if let Some(ctx) = weak_ctx.upgrade() {
            ctx.playback.unsubscribe(subscription);
        }
        if let Some(state) = owned.take()
            && let Some(task) = state.task.take()
        {
            task.abort();
        }
    });
    page
}

impl HistoryPage {
    /// The session's tracks, newest (the current one) first.
    fn show_session(&self) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        let mut tracks: Vec<TrackInfo> = ctx.playback.history();
        tracks.extend(ctx.playback.current());
        tracks.reverse();
        self.session_hint.set_visible(tracks.is_empty());
        let times = play_times(&tracks);
        let now = glib::DateTime::now_local().ok();
        let mut sections: Vec<(String, Rows)> = Vec::new();
        let mut last = None;
        for (track, time) in tracks.into_iter().zip(times) {
            let (day, note) = now
                .as_ref()
                .map_or((Day::Earlier, None), |now| when(now, time));
            // A track without a time stays with the ones played around it.
            let day = if note.is_none() {
                last.unwrap_or(day)
            } else {
                day
            };
            if last != Some(day) {
                last = Some(day);
                sections.push((day.title().to_owned(), Vec::new()));
            }
            if let Some((_, rows)) = sections.last_mut() {
                rows.push((track, note));
            }
        }
        self.list.set_sections(sections);
        self.update_empty();
    }

    fn update_empty(&self) {
        let empty = self.list.is_empty() && !self.remote_items.get();
        let loading = self.remote.visible_child_name().as_deref() == Some("loading");
        self.stack.set_visible_child_name(if empty && !loading {
            "empty"
        } else {
            "content"
        });
    }

    fn load_remote(self: &Rc<Self>) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        if let Some(task) = self.task.take() {
            task.abort();
        }
        let mut answers =
            ctx.api
                .load_cached(Base::V2, PATH.into(), FRESH_FOR, pages::parse_view_all);
        let weak = Rc::downgrade(self);
        let task = glib::spawn_future_local(async move {
            while let Some(answer) = answers.recv().await {
                let Some(state) = weak.upgrade() else {
                    return;
                };
                match answer {
                    Ok(section) => state.show_remote(&section),
                    Err(err) => state.remote_failed(&err),
                }
            }
        });
        self.task.replace(Some(task));
    }

    fn show_remote(&self, section: &Section) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        let cards = adw::WrapBox::builder()
            .child_spacing(12)
            .line_spacing(12)
            .build();
        let menu = ItemMenu::new(&ctx, &cards);
        for item in &section.items {
            cards.append(MediaCard::new(&ctx, item, Some(&menu)).widget());
        }
        let shown: gtk::Widget = if section.items.is_empty() {
            hint(tr("Nothing yet.")).upcast()
        } else {
            cards.upcast()
        };
        self.set_remote(&shown);
        self.remote_items.set(!section.items.is_empty());
        self.update_empty();
    }

    fn remote_failed(self: &Rc<Self>, err: &anyhow::Error) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        if tidal::is_auth_error(err) {
            ctx.report(tr("Couldn't load what you played"), err);
        }
        let row = gtk::Box::builder().spacing(12).build();
        let label =
            hint(&tr("Couldn't load this. {reason}").replace("{reason}", &describe_error(err)));
        label.set_hexpand(true);
        row.append(&label);
        let retry = gtk::Button::builder()
            .label(tr("Retry"))
            .valign(gtk::Align::Center)
            .build();
        let weak = Rc::downgrade(self);
        retry.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                state.remote.set_visible_child_name("loading");
                state.load_remote();
            }
        });
        row.append(&retry);
        self.set_remote(&row);
        self.update_empty();
    }

    fn set_remote(&self, widget: &impl IsA<gtk::Widget>) {
        if let Some(old) = self.remote.child_by_name("shown") {
            self.remote.remove(&old);
        }
        self.remote.add_named(widget, Some("shown"));
        self.remote.set_visible_child_name("shown");
    }
}

/// A line of text where the cards would be (which start 6 px in).
fn hint(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .wrap(true)
        .margin_start(section::CARD_INSET)
        .css_classes(["dimmed"])
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plays_group_by_day() {
        let local = |d: i32, h: i32, m: i32| {
            glib::DateTime::new(&glib::TimeZone::local(), 2026, 10, d, h, m, 0.0).unwrap()
        };
        let now = local(17, 15, 0);
        let at = |d, h, m| Some(local(d, h, m).to_unix());
        assert_eq!(when(&now, None), (Day::Earlier, None));
        assert_eq!(
            when(&now, Some(now.to_unix() - 30)).1.as_deref(),
            Some(tr("Just now"))
        );
        assert_eq!(
            when(&now, at(17, 14, 55)),
            (Day::Today, Some(tr("{n} min ago").replace("{n}", "5")))
        );
        assert_eq!(
            when(&now, at(17, 11, 30)),
            (Day::Today, Some(tr("{n} h ago").replace("{n}", "3")))
        );
        assert_eq!(
            when(&now, at(16, 20, 5)),
            (Day::Yesterday, Some("20:05".into()))
        );
        assert_eq!(when(&now, at(16, 14, 30)).0, Day::Yesterday);
        assert_eq!(when(&now, at(13, 9, 0)).0, Day::ThisWeek);
        assert_eq!(when(&now, at(3, 9, 0)).0, Day::Earlier);
    }
}
