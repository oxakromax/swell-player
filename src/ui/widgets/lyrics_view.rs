//! The Lyrics tab of the now-playing panel (docs/design.md §3.13).
//!
//! Lyrics are fetched off the main thread the first time the tab is shown
//! for a track and kept in memory per track id. Synced lyrics (LRC lines)
//! highlight the current line and keep it a third of the way down; scrolling
//! by hand pauses that for a few seconds (or until "Back to Current Line").
//! Clicking a line, or Enter on it, seeks there. The position is only
//! followed while the tab is visible and the player is playing.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk::glib;

use super::super::context::Ctx;
use super::np_common::{Subscription, Ticker, icons, subscribe};
use crate::api::Api;
use crate::i18n::tr;
use crate::playback::{Event, Playback};
use crate::runtime;
use crate::tidal::models::Lyrics;

/// Follows the position this often while synced lyrics are visible.
const TICK: Duration = Duration::from_millis(200);
/// Highlight a line slightly before it is sung.
const LEAD: f64 = 0.2;
/// Auto-scroll resumes this long after the user last scrolled.
const RESUME_AFTER: Duration = Duration::from_secs(5);
const SCROLL_DURATION: u32 = 400;
/// Ignore the engine's position this long after a seek from a line click.
const SEEK_SETTLE: Duration = Duration::from_millis(800);

/// What a track's lyrics turned out to be.
#[derive(Debug, PartialEq)]
enum Content {
    Synced {
        lines: Vec<(f64, String)>,
        provider: Option<String>,
        rtl: bool,
    },
    Plain {
        text: String,
        provider: Option<String>,
        rtl: bool,
    },
    None,
}

impl Content {
    fn from_lyrics(lyrics: Lyrics) -> Self {
        let provider = lyrics.lyrics_provider.filter(|p| !p.trim().is_empty());
        let rtl = lyrics.is_right_to_left;
        if let Some(lines) = lyrics.subtitles.as_deref().map(parse_lrc)
            && !lines.is_empty()
        {
            return Self::Synced {
                lines,
                provider,
                rtl,
            };
        }
        match lyrics.lyrics.filter(|text| !text.trim().is_empty()) {
            Some(text) => Self::Plain {
                text: text.trim().to_owned(),
                provider,
                rtl,
            },
            None => Self::None,
        }
    }
}

/// Parses LRC lines (`[mm:ss.xx] text`, possibly several time tags per
/// line), sorted by time. Metadata tags (`[ar:…]`) are skipped.
fn parse_lrc(text: &str) -> Vec<(f64, String)> {
    let mut lines = Vec::new();
    for raw in text.lines() {
        let mut rest = raw.trim();
        let mut times = Vec::new();
        while let Some(tag) = rest.strip_prefix('[') {
            let Some((inside, after)) = tag.split_once(']') else {
                break;
            };
            match parse_time(inside) {
                Some(time) => times.push(time),
                None => break,
            }
            rest = after.trim_start();
        }
        let words = rest.trim();
        for time in times {
            lines.push((time, words.to_owned()));
        }
    }
    lines.sort_by(|a, b| a.0.total_cmp(&b.0));
    lines
}

/// "mm:ss.xx" (or "mm:ss") to seconds.
fn parse_time(tag: &str) -> Option<f64> {
    let (minutes, seconds) = tag.split_once(':')?;
    let minutes: u32 = minutes.trim().parse().ok()?;
    let seconds: f64 = seconds.trim().replace(',', ".").parse().ok()?;
    (seconds.is_finite() && seconds >= 0.0).then(|| f64::from(minutes) * 60.0 + seconds)
}

struct Line {
    time: f64,
    row: gtk::ListBoxRow,
}

pub struct LyricsView {
    pub widget: gtk::Widget,
    this: Weak<Self>,
    ctx: Weak<Ctx>,
    api: Arc<Api>,
    playback: Rc<Playback>,
    stack: gtk::Stack,
    scrolled: gtk::ScrolledWindow,
    content: gtk::Box,
    list: gtk::ListBox,
    synced_credit: gtk::Label,
    plain: gtk::Label,
    plain_credit: gtk::Label,
    back: gtk::Button,
    animation: adw::TimedAnimation,
    lines: RefCell<Vec<Line>>,
    current: Cell<Option<usize>>,
    cache: RefCell<HashMap<u64, Rc<Content>>>,
    /// Track whose lyrics are on screen (or being fetched).
    shown: Cell<Option<u64>>,
    task: RefCell<Option<glib::JoinHandle<()>>>,
    follow: Cell<bool>,
    resume: RefCell<Option<glib::SourceId>>,
    seek_target: Cell<Option<(f64, Instant)>>,
    narrow: Cell<bool>,
    rtl: Cell<bool>,
    ticker: RefCell<Option<Ticker>>,
    _events: Subscription,
}

impl LyricsView {
    pub fn new(ctx: &Rc<Ctx>) -> Rc<Self> {
        let playback = &ctx.playback;
        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .vexpand(true)
            .build();

        let lyrics_icon = icons::lyrics();
        stack.add_named(
            &adw::StatusPage::builder()
                .icon_name(icons::track())
                .title(tr("Nothing Playing"))
                .css_classes(["compact"])
                .build(),
            Some("empty"),
        );
        stack.add_named(
            &adw::Spinner::builder()
                .width_request(32)
                .height_request(32)
                .halign(gtk::Align::Center)
                .valign(gtk::Align::Center)
                .build(),
            Some("loading"),
        );
        stack.add_named(
            &adw::StatusPage::builder()
                .icon_name(lyrics_icon)
                .title(tr("No Lyrics for This Track"))
                .css_classes(["compact"])
                .build(),
            Some("none"),
        );
        let retry = gtk::Button::builder()
            .label(tr("Retry"))
            .halign(gtk::Align::Center)
            .css_classes(["pill"])
            .build();
        stack.add_named(
            &adw::StatusPage::builder()
                .icon_name("network-offline-symbolic")
                .title(tr("Couldn't Load Lyrics"))
                .child(&retry)
                .css_classes(["compact"])
                .build(),
            Some("error"),
        );

        // Synced: a list of lines with a "Back to Current Line" button.
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["navigation-sidebar", "lyrics"])
            .build();
        list.update_property(&[gtk::accessible::Property::Label(tr("Lyrics"))]);
        let synced_credit = credit_label();
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(12)
            .margin_bottom(48)
            .build();
        content.append(&list);
        content.append(&synced_credit);
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&content)
            .vexpand(true)
            .build();
        let back = gtk::Button::builder()
            .label(tr("Back to Current Line"))
            .halign(gtk::Align::Center)
            .valign(gtk::Align::End)
            .margin_bottom(18)
            .css_classes(["pill", "osd"])
            .visible(false)
            .build();
        let overlay = gtk::Overlay::builder().child(&scrolled).build();
        overlay.add_overlay(&back);
        stack.add_named(&overlay, Some("synced"));

        // Plain: selectable text.
        let plain = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .selectable(true)
            .valign(gtk::Align::Start)
            .build();
        let plain_credit = credit_label();
        let plain_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(18)
            .margin_top(18)
            .margin_bottom(18)
            .margin_start(18)
            .margin_end(18)
            .build();
        plain_box.append(&plain);
        plain_box.append(&plain_credit);
        stack.add_named(
            &gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .child(&plain_box)
                .build(),
            Some("plain"),
        );

        let animation = adw::TimedAnimation::builder()
            .widget(&scrolled)
            .duration(SCROLL_DURATION)
            .easing(adw::Easing::EaseInOutCubic)
            .target(&adw::PropertyAnimationTarget::new(
                &scrolled.vadjustment(),
                "value",
            ))
            .build();

        let view = Rc::new_cyclic(|weak: &Weak<Self>| Self {
            widget: stack.clone().upcast(),
            this: weak.clone(),
            ctx: Rc::downgrade(ctx),
            api: ctx.api.clone(),
            playback: playback.clone(),
            stack,
            scrolled,
            content,
            list,
            synced_credit,
            plain,
            plain_credit,
            back,
            animation,
            lines: RefCell::default(),
            current: Cell::new(None),
            cache: RefCell::default(),
            shown: Cell::new(None),
            task: RefCell::default(),
            follow: Cell::new(true),
            resume: RefCell::default(),
            seek_target: Cell::new(None),
            narrow: Cell::new(false),
            rtl: Cell::new(false),
            ticker: RefCell::default(),
            _events: subscribe(playback, weak, |view, event| {
                if matches!(event, Event::Track(_)) {
                    view.track_changed();
                }
            }),
        });

        let weak = Rc::downgrade(&view);
        view.ticker
            .replace(Some(Ticker::new(&view.list, playback, TICK, {
                let weak = weak.clone();
                move || {
                    if let Some(view) = weak.upgrade() {
                        view.update_line(true);
                    }
                }
            })));
        view.stack.connect_map({
            let weak = weak.clone();
            move |_| {
                if let Some(view) = weak.upgrade() {
                    view.load();
                }
            }
        });
        retry.connect_clicked({
            let weak = weak.clone();
            move |_| {
                if let Some(view) = weak.upgrade() {
                    view.shown.set(None);
                    view.load();
                }
            }
        });
        view.list.connect_row_activated({
            let weak = weak.clone();
            move |_, row| {
                if let Some(view) = weak.upgrade() {
                    view.seek_to_line(row.index());
                }
            }
        });
        view.back.connect_clicked({
            let weak = weak.clone();
            move |_| {
                if let Some(view) = weak.upgrade() {
                    view.resume_follow();
                }
            }
        });

        // Anything the user does to scroll pauses following.
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
        scroll.connect_scroll({
            let weak = weak.clone();
            move |_, _, _| {
                if let Some(view) = weak.upgrade() {
                    view.user_scrolled();
                }
                glib::Propagation::Proceed
            }
        });
        view.scrolled.add_controller(scroll);
        let drag = gtk::GestureDrag::builder()
            .touch_only(true)
            .propagation_phase(gtk::PropagationPhase::Capture)
            .build();
        drag.connect_drag_update({
            let weak = weak.clone();
            move |_, _, dy| {
                if dy.abs() > 8.0
                    && let Some(view) = weak.upgrade()
                {
                    view.user_scrolled();
                }
            }
        });
        view.scrolled.add_controller(drag);
        let scrollbar_click = gtk::GestureClick::builder()
            .propagation_phase(gtk::PropagationPhase::Capture)
            .build();
        scrollbar_click.connect_pressed(move |_, _, _, _| {
            if let Some(view) = weak.upgrade() {
                view.user_scrolled();
            }
        });
        view.scrolled.vscrollbar().add_controller(scrollbar_click);
        view
    }

    /// Narrow layout: centred lines.
    pub fn set_narrow(&self, narrow: bool) {
        self.narrow.set(narrow);
        self.align_lines();
    }

    fn track_changed(&self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.shown.set(None);
        self.clear_lines();
        if let Some(view) = self.this.upgrade() {
            view.load();
        }
    }

    /// Shows the current track's lyrics, fetching them if needed. Only
    /// while visible: tracks that play with the tab closed cost nothing.
    fn load(self: &Rc<Self>) {
        if !self.stack.is_mapped() {
            return;
        }
        let Some(track) = self.playback.current() else {
            self.shown.set(None);
            self.clear_lines();
            self.stack.set_visible_child_name("empty");
            return;
        };
        if self.shown.get() == Some(track.id) {
            self.scroll_to_current_soon();
            return;
        }
        self.shown.set(Some(track.id));
        if let Some(task) = self.task.take() {
            task.abort();
        }
        let cached = self.cache.borrow().get(&track.id).cloned();
        if let Some(content) = cached {
            self.display(&content);
            return;
        }
        self.clear_lines();
        self.stack.set_visible_child_name("loading");
        let api = self.api.clone();
        let weak = Rc::downgrade(self);
        let id = track.id;
        let task = glib::spawn_future_local(async move {
            let result = runtime::spawn(async move { api.lyrics(id).await }).await;
            let Some(view) = weak.upgrade() else { return };
            view.task.take();
            let content = match result {
                Ok(lyrics) => Content::from_lyrics(lyrics),
                Err(err) if crate::tidal::is_not_found(&err) => Content::None,
                Err(err) => {
                    if crate::tidal::is_auth_error(&err)
                        && let Some(ctx) = view.ctx.upgrade()
                    {
                        ctx.report(tr("Couldn't load lyrics"), &err);
                    }
                    log::warn!("lyrics of track {id}: {err:#}");
                    if view.shown.get() == Some(id) {
                        view.stack.set_visible_child_name("error");
                    }
                    return;
                }
            };
            let content = Rc::new(content);
            view.cache.borrow_mut().insert(id, content.clone());
            if view.shown.get() == Some(id) {
                view.display(&content);
            }
        });
        self.task.replace(Some(task));
    }

    fn display(&self, content: &Content) {
        self.clear_lines();
        match content {
            Content::None => self.stack.set_visible_child_name("none"),
            Content::Plain {
                text,
                provider,
                rtl,
            } => {
                self.rtl.set(*rtl);
                self.plain.set_label(text);
                set_credit(&self.plain_credit, provider.as_deref());
                self.align_lines();
                self.stack.set_visible_child_name("plain");
            }
            Content::Synced {
                lines,
                provider,
                rtl,
            } => {
                self.rtl.set(*rtl);
                let mut rows = Vec::with_capacity(lines.len());
                for (time, text) in lines {
                    let label = gtk::Label::builder()
                        .label(if text.is_empty() {
                            "♪"
                        } else {
                            text.as_str()
                        })
                        .wrap(true)
                        .wrap_mode(gtk::pango::WrapMode::WordChar)
                        .css_classes(["title-3", "lyrics-line", "synced"])
                        .build();
                    let row = gtk::ListBoxRow::builder().child(&label).build();
                    self.list.append(&row);
                    rows.push(Line { time: *time, row });
                }
                self.lines.replace(rows);
                set_credit(&self.synced_credit, provider.as_deref());
                self.align_lines();
                self.follow.set(true);
                self.back.set_visible(false);
                self.stack.set_visible_child_name("synced");
                self.update_line(false);
                self.scroll_to_current_soon();
            }
        }
    }

    fn clear_lines(&self) {
        self.animation.pause();
        self.current.set(None);
        for line in self.lines.take() {
            self.list.remove(&line.row);
        }
    }

    fn align_lines(&self) {
        let (xalign, justify) = if self.narrow.get() {
            (0.5, gtk::Justification::Center)
        } else {
            (0.0, gtk::Justification::Left)
        };
        let direction = if self.rtl.get() {
            gtk::TextDirection::Rtl
        } else {
            gtk::TextDirection::Ltr
        };
        for line in self.lines.borrow().iter() {
            if let Some(label) = line.row.child().and_downcast::<gtk::Label>() {
                label.set_xalign(xalign);
                label.set_justify(justify);
                label.set_direction(direction);
            }
        }
        self.plain.set_direction(direction);
        self.plain.set_xalign(xalign);
        self.plain.set_justify(justify);
    }

    /// Highlights the line being sung; scrolls to it when following.
    fn update_line(&self, animate: bool) {
        let index = {
            let lines = self.lines.borrow();
            if lines.is_empty() {
                return;
            }
            let position = match self.seek_target.get() {
                Some((target, until)) if Instant::now() < until => target,
                _ => {
                    self.seek_target.set(None);
                    self.playback.position().unwrap_or(0.0)
                }
            } + LEAD;
            lines
                .partition_point(|line| line.time <= position)
                .checked_sub(1)
        };
        if index == self.current.get() {
            return;
        }
        let lines = self.lines.borrow();
        if let Some(old) = self.current.get().and_then(|i| lines.get(i)) {
            set_current(&old.row, false);
        }
        if let Some(new) = index.and_then(|i| lines.get(i)) {
            set_current(&new.row, true);
        }
        drop(lines);
        self.current.set(index);
        if self.follow.get() {
            self.scroll_to_current(animate);
        }
    }

    /// Puts the current line a third of the way down.
    fn scroll_to_current(&self, animate: bool) {
        let lines = self.lines.borrow();
        let row = match self.current.get().and_then(|i| lines.get(i)) {
            Some(line) => &line.row,
            None => match lines.first() {
                Some(line) => &line.row,
                None => return,
            },
        };
        let Some(point) = row.compute_point(&self.content, &gtk::graphene::Point::zero()) else {
            return;
        };
        let adjustment = self.scrolled.vadjustment();
        let page = adjustment.page_size();
        let target = (f64::from(point.y()) + f64::from(row.height()) / 2.0 - page / 3.0).clamp(
            adjustment.lower(),
            (adjustment.upper() - page).max(adjustment.lower()),
        );
        self.animation.pause();
        if animate && (target - adjustment.value()).abs() > 1.0 {
            self.animation.set_value_from(adjustment.value());
            self.animation.set_value_to(target);
            self.animation.play();
        } else {
            adjustment.set_value(target);
        }
    }

    /// New lines need a layout pass before they can be scrolled to: wait
    /// two frames.
    fn scroll_to_current_soon(&self) {
        let frames = Cell::new(0);
        let weak = self.this.clone();
        self.list.add_tick_callback(move |_, _| {
            frames.set(frames.get() + 1);
            if frames.get() < 2 {
                return glib::ControlFlow::Continue;
            }
            if let Some(view) = weak.upgrade()
                && view.follow.get()
            {
                view.scroll_to_current(false);
            }
            glib::ControlFlow::Break
        });
    }

    fn seek_to_line(&self, index: i32) {
        let time = {
            let lines = self.lines.borrow();
            let Some(line) = usize::try_from(index).ok().and_then(|i| lines.get(i)) else {
                return;
            };
            line.time
        };
        self.playback.seek(time);
        self.seek_target
            .set(Some((time, Instant::now() + SEEK_SETTLE)));
        self.follow.set(true);
        self.back.set_visible(false);
        if let Some(id) = self.resume.take() {
            id.remove();
        }
        self.update_line(true);
    }

    fn user_scrolled(&self) {
        if self.lines.borrow().is_empty() {
            return;
        }
        self.animation.pause();
        self.follow.set(false);
        self.back.set_visible(true);
        if let Some(id) = self.resume.take() {
            id.remove();
        }
        let weak = self.this.clone();
        let id = glib::timeout_add_local_once(RESUME_AFTER, move || {
            if let Some(view) = weak.upgrade() {
                // This source is done; forget its id instead of removing it.
                view.resume.take();
                view.resume_follow();
            }
        });
        self.resume.replace(Some(id));
    }

    fn resume_follow(&self) {
        if let Some(id) = self.resume.take() {
            id.remove();
        }
        self.follow.set(true);
        self.back.set_visible(false);
        self.scroll_to_current(true);
    }
}

impl Drop for LyricsView {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        if let Some(id) = self.resume.take() {
            id.remove();
        }
    }
}

fn credit_label() -> gtk::Label {
    gtk::Label::builder()
        .wrap(true)
        .css_classes(["caption", "dimmed"])
        .margin_start(18)
        .margin_end(18)
        .visible(false)
        .build()
}

fn set_credit(label: &gtk::Label, provider: Option<&str>) {
    match provider {
        Some(provider) => {
            label.set_label(&tr("Lyrics provided by {provider}").replace("{provider}", provider));
            label.set_visible(true);
        }
        None => label.set_visible(false),
    }
}

fn set_current(row: &gtk::ListBoxRow, current: bool) {
    if let Some(label) = row.child() {
        if current {
            label.add_css_class("current");
        } else {
            label.remove_css_class("current");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lrc() {
        let lines = parse_lrc(
            "[ar:Someone]\n[00:12.50] Hello\n[01:02.00][00:05.25] Again\n[00:20.00]\nnot a line",
        );
        assert_eq!(
            lines,
            vec![
                (5.25, "Again".to_owned()),
                (12.5, "Hello".to_owned()),
                (20.0, String::new()),
                (62.0, "Again".to_owned()),
            ]
        );
    }

    #[test]
    fn content_prefers_synced() {
        let lyrics = Lyrics {
            lyrics: Some("Hello".into()),
            subtitles: Some("[00:01.00] Hello".into()),
            lyrics_provider: Some("Provider".into()),
            is_right_to_left: false,
        };
        assert!(matches!(
            Content::from_lyrics(lyrics),
            Content::Synced { .. }
        ));
        let empty = Lyrics {
            lyrics: Some("  ".into()),
            subtitles: None,
            lyrics_provider: None,
            is_right_to_left: false,
        };
        assert_eq!(Content::from_lyrics(empty), Content::None);
    }
}
