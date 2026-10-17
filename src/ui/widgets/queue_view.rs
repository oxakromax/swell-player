//! The Queue tab of the now-playing panel (docs/design.md §3.13): what is
//! playing, then the tracks queued by hand ("Next in Queue"), then the rest
//! of the context ("Next From: <source>").
//!
//! A virtualised `ListView` over a flattened model with one store per
//! section; updates are applied as minimal splices so the scroll position
//! and focus survive. Click or Enter jumps to a track; ✕ or Delete removes
//! it; drag (or Alt+↑/↓) reorders; "Clear" empties what is upcoming, with
//! Undo.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use super::super::context::Ctx;
use super::super::nav::Route;
use super::np_common::{Cover, Subscription, escape, icons, subscribe};
use crate::covers::CoverLoader;
use crate::model::{TrackInfo, format_duration};
use crate::playback::{Event, Playback};

const COVER_SIZE: i32 = 40;
const ROW_HEIGHT: i32 = 54;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    Now,
    Next,
    From,
}

/// What a list item holds.
struct Entry {
    section: Section,
    track: TrackInfo,
}

fn entry_object(section: Section, track: TrackInfo) -> glib::BoxedAnyObject {
    glib::BoxedAnyObject::new(Entry { section, track })
}

fn with_entry<R>(object: &glib::Object, f: impl FnOnce(&Entry) -> R) -> Option<R> {
    let boxed = object.downcast_ref::<glib::BoxedAnyObject>()?;
    let entry = boxed.try_borrow::<Entry>().ok()?;
    Some(f(&entry))
}

/// The widgets of one recycled row.
struct Row {
    widget: gtk::Box,
    item: glib::WeakRef<gtk::ListItem>,
    cover: Cover,
    title: gtk::Label,
    artists: gtk::Label,
    duration: gtk::Label,
    handle: gtk::Image,
    remove: gtk::Button,
}

pub struct QueueView {
    pub widget: gtk::Widget,
    this: Weak<Self>,
    ctx: Weak<Ctx>,
    playback: Rc<Playback>,
    covers: Rc<CoverLoader>,
    stack: gtk::Stack,
    header: gtk::Box,
    source: gtk::Label,
    clear: gtk::Button,
    list: gtk::ListView,
    stores: [gio::ListStore; 3],
    source_title: RefCell<String>,
    rows: RefCell<Vec<Rc<Row>>>,
    _events: Subscription,
}

impl QueueView {
    pub fn new(ctx: &Rc<Ctx>) -> Rc<Self> {
        let playback = &ctx.playback;
        let source = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .single_line_mode(true)
            .use_markup(true)
            .css_classes(["caption", "dimmed", "np-link"])
            .build();
        let clear = gtk::Button::builder()
            .label("Clear")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .tooltip_text("Clear Queue")
            .build();
        let header = gtk::Box::builder()
            .spacing(12)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(18)
            .margin_end(12)
            .build();
        header.append(&source);
        header.append(&clear);

        let stores: [gio::ListStore; 3] =
            std::array::from_fn(|_| gio::ListStore::new::<glib::BoxedAnyObject>());
        let sections = gio::ListStore::new::<gio::ListStore>();
        for store in &stores {
            sections.append(store);
        }
        let model = gtk::FlattenListModel::new(Some(sections));
        let list = gtk::ListView::builder()
            .model(&gtk::NoSelection::new(Some(model)))
            .single_click_activate(true)
            .css_classes(["navigation-sidebar", "queue"])
            .build();
        list.update_property(&[gtk::accessible::Property::Label("Queue")]);
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&list)
            .vexpand(true)
            .build();
        let list_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        list_box.append(&header);
        list_box.append(&scrolled);

        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .vexpand(true)
            .build();
        stack.add_named(&list_box, Some("list"));
        stack.add_named(
            &adw::StatusPage::builder()
                .icon_name(icons::queue())
                .title("Queue Is Empty")
                .description("Use “Add to Queue” on any track, album or playlist.")
                .css_classes(["compact"])
                .build(),
            Some("empty"),
        );

        let view = Rc::new_cyclic(|weak: &Weak<Self>| Self {
            widget: stack.clone().upcast(),
            this: weak.clone(),
            ctx: Rc::downgrade(ctx),
            playback: playback.clone(),
            covers: ctx.covers.clone(),
            stack,
            header,
            source,
            clear,
            list,
            stores,
            source_title: RefCell::default(),
            rows: RefCell::default(),
            _events: subscribe(playback, weak, |view, event| {
                if matches!(event, Event::Queue | Event::Track(_)) {
                    view.refresh();
                }
            }),
        });
        view.list.set_factory(Some(&view.row_factory()));
        view.list.set_header_factory(Some(&view.header_factory()));

        let weak = Rc::downgrade(&view);
        view.list.connect_activate({
            let weak = weak.clone();
            move |_, position| {
                if let Some(view) = weak.upgrade()
                    && let Some(index) = view.upcoming_index(position)
                {
                    view.playback.jump_to(index);
                }
            }
        });
        view.clear.connect_clicked({
            let weak = weak.clone();
            move |_| {
                if let Some(view) = weak.upgrade() {
                    view.clear_upcoming();
                }
            }
        });
        view.source.connect_activate_link({
            let weak = weak.clone();
            move |_, _| {
                if let Some(view) = weak.upgrade()
                    && let Some(ctx) = view.ctx.upgrade()
                    && let Some(route) = view
                        .playback
                        .source()
                        .and_then(|source| source.target)
                        .and_then(|target| Route::from_target(&target))
                {
                    ctx.navigate(route);
                }
                glib::Propagation::Stop
            }
        });
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed({
            let weak = weak.clone();
            move |_, key, _, modifiers| match weak.upgrade() {
                Some(view) if view.on_key(key, modifiers) => glib::Propagation::Stop,
                _ => glib::Propagation::Proceed,
            }
        });
        view.list.add_controller(keys);
        // Opening the tab shows the current track.
        view.list.connect_map(move |list| {
            if weak
                .upgrade()
                .is_some_and(|view| view.stores[0].n_items() > 0)
            {
                list.scroll_to(0, gtk::ListScrollFlags::NONE, None);
            }
        });
        view.refresh();
        view
    }

    /// Number of items before the upcoming ones (the current track).
    fn now_len(&self) -> u32 {
        self.stores[0].n_items()
    }

    /// Index into `Playback::upcoming` of the item at `position`.
    fn upcoming_index(&self, position: u32) -> Option<usize> {
        let now = self.now_len();
        let total = now + self.stores[1].n_items() + self.stores[2].n_items();
        (position >= now && position < total).then(|| (position - now) as usize)
    }

    fn refresh(&self) {
        let current = self.playback.current();
        let upcoming = self.playback.upcoming();
        let up_next = self.playback.up_next_len().min(upcoming.len());
        let source = self.playback.source();
        let title = source.as_ref().map(|s| s.title.clone()).unwrap_or_default();
        let title_changed = *self.source_title.borrow() != title;
        self.source_title.replace(title.clone());

        let mut rest = upcoming;
        let next = rest.drain(..up_next).collect::<Vec<_>>();
        update_store(
            &self.stores[0],
            Section::Now,
            current.clone().into_iter().collect(),
            false,
        );
        update_store(&self.stores[1], Section::Next, next, false);
        update_store(&self.stores[2], Section::From, rest, title_changed);

        let empty = current.is_none() && self.stores[1].n_items() + self.stores[2].n_items() == 0;
        self.stack
            .set_visible_child_name(if empty { "empty" } else { "list" });
        let has_upcoming = self.stores[1].n_items() + self.stores[2].n_items() > 0;
        self.clear.set_visible(has_upcoming);
        match source.filter(|s| !s.title.is_empty()) {
            Some(source) => {
                let title = escape(&source.title);
                let linked = source
                    .target
                    .as_ref()
                    .and_then(Route::from_target)
                    .is_some();
                self.source.set_markup(&if linked {
                    format!("Playing from <a href=\"source\">{title}</a>")
                } else {
                    format!("Playing from {title}")
                });
                self.source.set_tooltip_text(Some(&source.title));
            }
            None => self.source.set_label(""),
        }
        self.header.set_visible(has_upcoming || !title.is_empty());
    }

    fn clear_upcoming(&self) {
        let saved = self.playback.upcoming();
        if saved.is_empty() {
            return;
        }
        self.playback.clear_upcoming();
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        // Undo re-queues everything by hand: the context itself cannot be
        // restored through the playback API.
        let playback = Rc::downgrade(&self.playback);
        ctx.toast_with_undo("Queue cleared", move || {
            if let Some(playback) = playback.upgrade() {
                playback.add_to_queue(saved.clone());
            }
        });
    }

    fn row_of(&self, widget: &gtk::Widget) -> Option<Rc<Row>> {
        self.rows
            .borrow()
            .iter()
            .find(|row| row.widget.upcast_ref::<gtk::Widget>() == widget)
            .cloned()
    }

    /// Position of the row with keyboard focus.
    fn focused_position(&self) -> Option<u32> {
        let row_widget = self.list.focus_child()?;
        let child = row_widget.first_child()?;
        let row = self.row_of(&child)?;
        let item = row.item.upgrade()?;
        let position = item.position();
        (position != gtk::INVALID_LIST_POSITION).then_some(position)
    }

    fn on_key(&self, key: gdk::Key, modifiers: gdk::ModifierType) -> bool {
        let Some(position) = self.focused_position() else {
            return false;
        };
        let Some(index) = self.upcoming_index(position) else {
            return false;
        };
        let now = self.now_len();
        let upcoming = (self.stores[1].n_items() + self.stores[2].n_items()) as usize;
        let alt = modifiers.contains(gdk::ModifierType::ALT_MASK);
        let target = match key {
            gdk::Key::Delete | gdk::Key::KP_Delete if modifiers.is_empty() => {
                self.playback.remove(index);
                let left = upcoming.saturating_sub(1);
                if left == 0 {
                    return true;
                }
                index.min(left - 1)
            }
            gdk::Key::Up | gdk::Key::KP_Up if alt && index > 0 => {
                self.playback.move_item(index, index - 1);
                index - 1
            }
            gdk::Key::Down | gdk::Key::KP_Down if alt && index + 1 < upcoming => {
                self.playback.move_item(index, index + 1);
                index + 1
            }
            _ => return false,
        };
        self.list
            .scroll_to(now + target as u32, gtk::ListScrollFlags::FOCUS, None);
        true
    }

    /// Moves the dragged item `from` before (or after) item `onto`.
    fn drop_on(&self, from: usize, onto: usize, after: bool) {
        let before = onto + usize::from(after);
        let to = if from < before { before - 1 } else { before };
        if to != from {
            self.playback.move_item(from, to);
        }
    }

    fn row_factory(&self) -> gtk::SignalListItemFactory {
        let factory = gtk::SignalListItemFactory::new();
        let weak = self.this.clone();
        factory.connect_setup({
            let weak = weak.clone();
            move |_, object| {
                let Some(item) = object.downcast_ref::<gtk::ListItem>() else {
                    return;
                };
                if let Some(view) = weak.upgrade() {
                    let row = view.build_row(item);
                    item.set_child(Some(&row.widget));
                    view.rows.borrow_mut().push(row);
                }
            }
        });
        factory.connect_bind({
            let weak = weak.clone();
            move |_, object| {
                let Some(item) = object.downcast_ref::<gtk::ListItem>() else {
                    return;
                };
                if let Some(view) = weak.upgrade() {
                    view.bind_row(item);
                }
            }
        });
        factory.connect_unbind({
            let weak = weak.clone();
            move |_, object| {
                let Some(item) = object.downcast_ref::<gtk::ListItem>() else {
                    return;
                };
                if let Some(view) = weak.upgrade()
                    && let Some(row) = item.child().and_then(|c| view.row_of(&c))
                {
                    row.cover.set(&view.covers, None);
                }
            }
        });
        factory.connect_teardown(move |_, object| {
            let Some(item) = object.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            if let Some(view) = weak.upgrade()
                && let Some(child) = item.child()
            {
                view.rows
                    .borrow_mut()
                    .retain(|row| row.widget.upcast_ref::<gtk::Widget>() != &child);
            }
        });
        factory
    }

    fn build_row(&self, item: &gtk::ListItem) -> Rc<Row> {
        let cover = Cover::new(COVER_SIZE);
        let label = |classes: &[&str]| {
            gtk::Label::builder()
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .single_line_mode(true)
                .css_classes(classes.to_vec())
                .build()
        };
        let title = label(&["track-title"]);
        let artists = label(&["dimmed"]);
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .valign(gtk::Align::Center)
            .hexpand(true)
            .build();
        text.append(&title);
        text.append(&artists);
        let duration = gtk::Label::builder()
            .css_classes(["numeric", "dimmed"])
            .valign(gtk::Align::Center)
            .build();
        let handle = gtk::Image::builder()
            .icon_name(icons::drag_handle())
            .css_classes(["dimmed"])
            .tooltip_text("Drag to Reorder")
            .accessible_role(gtk::AccessibleRole::Presentation)
            .build();
        let remove = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(["flat", "circular", "row-more"])
            .tooltip_text("Remove from Queue")
            .build();
        remove.update_property(&[gtk::accessible::Property::Label("Remove from Queue")]);
        let widget = gtk::Box::builder()
            .spacing(12)
            .height_request(ROW_HEIGHT)
            .css_classes(["track-row"])
            .build();
        widget.append(&cover.image);
        widget.append(&text);
        widget.append(&duration);
        widget.append(&remove);
        widget.append(&handle);

        let row = Rc::new(Row {
            widget,
            item: item.downgrade(),
            cover,
            title,
            artists,
            duration,
            handle,
            remove,
        });
        let weak_view = self.this.clone();
        let weak_row = Rc::downgrade(&row);
        let index_of = {
            let weak_view = weak_view.clone();
            let weak_row = weak_row.clone();
            move || {
                let view = weak_view.upgrade()?;
                let item = weak_row.upgrade()?.item.upgrade()?;
                view.upcoming_index(item.position())
            }
        };
        row.remove.connect_clicked({
            let index_of = index_of.clone();
            let weak_view = weak_view.clone();
            move |_| {
                if let (Some(view), Some(index)) = (weak_view.upgrade(), index_of()) {
                    view.playback.remove(index);
                }
            }
        });

        // Drag to reorder: the payload is the upcoming index.
        let drag = gtk::DragSource::new();
        drag.set_actions(gdk::DragAction::MOVE);
        drag.connect_prepare({
            let index_of = index_of.clone();
            move |_, _, _| {
                let index = u32::try_from(index_of()?).ok()?;
                Some(gdk::ContentProvider::for_value(&index.to_value()))
            }
        });
        drag.connect_drag_begin({
            let weak_row = weak_row.clone();
            move |source, _| {
                if let Some(row) = weak_row.upgrade() {
                    let paintable = gtk::WidgetPaintable::new(Some(&row.widget));
                    source.set_icon(Some(&paintable), 0, 0);
                }
            }
        });
        row.widget.add_controller(drag);

        let drop = gtk::DropTarget::new(u32::static_type(), gdk::DragAction::MOVE);
        let set_marker = |widget: &gtk::Box, after: Option<bool>| {
            widget.remove_css_class("drop-above");
            widget.remove_css_class("drop-below");
            match after {
                Some(true) => widget.add_css_class("drop-below"),
                Some(false) => widget.add_css_class("drop-above"),
                None => {}
            }
        };
        drop.connect_motion({
            let weak_row = weak_row.clone();
            let index_of = index_of.clone();
            move |_, _, y| {
                let Some(row) = weak_row.upgrade() else {
                    return gdk::DragAction::empty();
                };
                if index_of().is_none() {
                    set_marker(&row.widget, None);
                    return gdk::DragAction::empty();
                }
                set_marker(&row.widget, Some(y > f64::from(row.widget.height()) / 2.0));
                gdk::DragAction::MOVE
            }
        });
        drop.connect_leave({
            let weak_row = weak_row.clone();
            move |_| {
                if let Some(row) = weak_row.upgrade() {
                    set_marker(&row.widget, None);
                }
            }
        });
        drop.connect_drop(move |_, value, _, y| {
            let Some(row) = weak_row.upgrade() else {
                return false;
            };
            set_marker(&row.widget, None);
            let (Some(view), Some(onto), Ok(from)) =
                (weak_view.upgrade(), index_of(), value.get::<u32>())
            else {
                return false;
            };
            let after = y > f64::from(row.widget.height()) / 2.0;
            view.drop_on(from as usize, onto, after);
            true
        });
        row.widget.add_controller(drop);
        row
    }

    fn bind_row(&self, item: &gtk::ListItem) {
        let Some(row) = item.child().and_then(|c| self.row_of(&c)) else {
            return;
        };
        let Some(object) = item.item() else { return };
        with_entry(&object, |entry| {
            let track = &entry.track;
            let now = entry.section == Section::Now;
            row.cover.set(&self.covers, track.cover.as_deref());
            row.title.set_label(&track.title);
            row.title.set_tooltip_text(Some(&track.title));
            let artists = track.artist_names();
            row.artists.set_label(&artists);
            let duration = format_duration(track.duration.into());
            row.duration.set_label(&duration);
            row.remove.set_visible(!now);
            row.handle.set_visible(!now);
            if now {
                row.widget.add_css_class("playing");
            } else {
                row.widget.remove_css_class("playing");
            }
            item.set_activatable(!now);
            item.set_accessible_label(&format!(
                "{}, {artists}, {duration}{}",
                track.title,
                if now { ", playing" } else { "" }
            ));
        });
    }

    fn header_factory(&self) -> gtk::SignalListItemFactory {
        let factory = gtk::SignalListItemFactory::new();
        let weak = self.this.clone();
        factory.connect_setup(move |_, object| {
            let Some(header) = object.downcast_ref::<gtk::ListHeader>() else {
                return;
            };
            let label = gtk::Label::builder()
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .margin_top(12)
                .margin_bottom(6)
                .margin_start(12)
                .margin_end(12)
                .css_classes(["heading"])
                .build();
            header.set_child(Some(&label));
            let weak = weak.clone();
            let update = move |header: &gtk::ListHeader| {
                let (Some(view), Some(label), Some(object)) = (
                    weak.upgrade(),
                    header.child().and_downcast::<gtk::Label>(),
                    header.item(),
                ) else {
                    return;
                };
                let section = with_entry(&object, |entry| entry.section);
                label.set_label(&match section {
                    Some(Section::Now) => "Now Playing".to_owned(),
                    Some(Section::Next) => "Next in Queue".to_owned(),
                    _ => match view.source_title.borrow().as_str() {
                        "" => "Next".to_owned(),
                        title => format!("Next From: {title}"),
                    },
                });
            };
            update(header);
            header.connect_item_notify(update);
        });
        factory
    }
}

/// Makes `store` hold `tracks`, splicing only the part that changed.
fn update_store(store: &gio::ListStore, section: Section, tracks: Vec<TrackInfo>, force: bool) {
    let old: Vec<u64> = (0..store.n_items())
        .filter_map(|i| store.item(i))
        .map(|object| with_entry(&object, |entry| entry.track.id).unwrap_or(0))
        .collect();
    let (prefix, suffix) = if force {
        (0, 0)
    } else {
        let prefix = old
            .iter()
            .zip(&tracks)
            .take_while(|(old, new)| **old == new.id)
            .count();
        let room = old.len().min(tracks.len()) - prefix;
        let suffix = old
            .iter()
            .rev()
            .zip(tracks.iter().rev())
            .take(room)
            .take_while(|(old, new)| **old == new.id)
            .count();
        (prefix, suffix)
    };
    let removed = old.len() - prefix - suffix;
    let added: Vec<glib::BoxedAnyObject> = tracks[prefix..tracks.len() - suffix]
        .iter()
        .map(|track| entry_object(section, track.clone()))
        .collect();
    if removed == 0 && added.is_empty() {
        return;
    }
    store.splice(prefix as u32, removed as u32, &added);
}
