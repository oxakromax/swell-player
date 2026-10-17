//! The favourite-tracks page: a virtualised `gtk::ListView` fed page by page.

use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

use super::track_row::TrackRow;
use crate::covers::CoverLoader;
use crate::model::TrackInfo;

pub struct TrackList {
    /// The scrolled window, with the list view as its direct child so only
    /// the rows on screen (plus GTK's small overscan) exist as widgets.
    pub widget: gtk::ScrolledWindow,
    view: gtk::ListView,
    store: gio::ListStore,
}

impl TrackList {
    pub fn new(covers: Rc<CoverLoader>) -> Self {
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();

        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            list_item(item).set_child(Some(&TrackRow::new()));
        });
        factory.connect_bind({
            let covers = covers.clone();
            move |_, item| {
                let item = list_item(item);
                let (Some(row), Some(object)) = (
                    item.child().and_downcast::<TrackRow>(),
                    item.item().and_downcast::<glib::BoxedAnyObject>(),
                ) else {
                    return;
                };
                row.bind(&object.borrow::<TrackInfo>(), item.position(), &covers);
            }
        });
        factory.connect_unbind(|_, item| {
            if let Some(row) = list_item(item).child().and_downcast::<TrackRow>() {
                row.unbind();
            }
        });

        let view = gtk::ListView::builder()
            .model(&gtk::NoSelection::new(Some(store.clone())))
            .factory(&factory)
            .build();
        let widget = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&view)
            .build();

        // Tell the cover pool which rows are on screen. Rows have one height,
        // so the scroll fraction maps straight to a list position.
        let adjustment = widget.vadjustment();
        let update_focus = {
            let store = store.clone();
            move |adjustment: &gtk::Adjustment| {
                let upper = adjustment.upper();
                if upper > 0.0 {
                    let middle = (adjustment.value() + adjustment.page_size() / 2.0) / upper;
                    covers.set_focus((middle * f64::from(store.n_items())) as u32);
                }
            }
        };
        adjustment.connect_value_changed(update_focus.clone());
        adjustment.connect_changed(update_focus);

        Self {
            widget,
            view,
            store,
        }
    }

    pub fn append(&self, tracks: Vec<TrackInfo>) {
        let objects: Vec<_> = tracks.into_iter().map(glib::BoxedAnyObject::new).collect();
        self.store.extend_from_slice(&objects);
    }

    pub fn replace(&self, tracks: Vec<TrackInfo>) {
        let objects: Vec<_> = tracks.into_iter().map(glib::BoxedAnyObject::new).collect();
        self.store.splice(0, self.store.n_items(), &objects);
    }

    /// Double click or Enter on a row.
    pub fn connect_track_activated(&self, on_activate: impl Fn(TrackInfo) + 'static) {
        let store = self.store.clone();
        self.view.connect_activate(move |_, position| {
            if let Some(object) = store.item(position).and_downcast::<glib::BoxedAnyObject>() {
                on_activate(object.borrow::<TrackInfo>().clone());
            }
        });
    }

    pub fn first_where(&self, predicate: impl Fn(&TrackInfo) -> bool) -> Option<TrackInfo> {
        (0..self.store.n_items())
            .filter_map(|position| self.store.item(position).and_downcast::<glib::BoxedAnyObject>())
            .map(|object| object.borrow::<TrackInfo>().clone())
            .find(|track| predicate(track))
    }

    pub fn len(&self) -> u32 {
        self.store.n_items()
    }
}

fn list_item(item: &glib::Object) -> &gtk::ListItem {
    item.downcast_ref()
        .expect("the factory only receives list items")
}
