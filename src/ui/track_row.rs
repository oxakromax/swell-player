//! One row of a track list. Rows are recycled by `gtk::ListView`: `bind`
//! fills a row with a track, `unbind` releases what it was showing.

use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::covers::{CoverLoader, Image};
use crate::model::{TrackInfo, format_duration};

/// Logical pixels; the 80 px download stays sharp on 1.5× screens.
const COVER_DISPLAY_SIZE: i32 = 48;
const COVER_PLACEHOLDER: &str = "audio-x-generic-symbolic";

mod imp {
    use std::cell::RefCell;

    use super::*;
    use crate::covers::Ticket;

    #[derive(Default)]
    pub struct TrackRow {
        pub cover: gtk::Image,
        pub title: gtk::Label,
        pub artists: gtk::Label,
        pub album: gtk::Label,
        pub hires: gtk::Label,
        pub duration: gtk::Label,
        pub cover_ticket: RefCell<Option<Ticket>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TrackRow {
        const NAME: &'static str = "SwellTrackRow";
        type Type = super::TrackRow;
        type ParentType = gtk::Box;
    }

    impl ObjectImpl for TrackRow {
        fn constructed(&self) {
            self.parent_constructed();
            let row = self.obj();
            row.set_spacing(12);
            row.set_margin_top(6);
            row.set_margin_bottom(6);
            row.set_margin_start(12);
            row.set_margin_end(12);

            self.cover.set_pixel_size(COVER_DISPLAY_SIZE);
            self.cover.add_css_class("card");
            self.cover.set_overflow(gtk::Overflow::Hidden);

            for label in [&self.title, &self.artists, &self.album] {
                label.set_xalign(0.0);
                label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                label.set_single_line_mode(true);
            }
            self.artists.add_css_class("dim-label");
            self.album.add_css_class("dim-label");

            let text = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .valign(gtk::Align::Center)
                .build();
            text.append(&self.title);
            text.append(&self.artists);

            // Equal widths for the title and album columns, so albums line up
            // from row to row whatever the length of the titles.
            let columns = gtk::Box::builder()
                .homogeneous(true)
                .spacing(12)
                .hexpand(true)
                .build();
            columns.append(&text);
            columns.append(&self.album);

            // Always allocated, empty when not Hi-Res, so durations line up.
            self.hires.set_width_chars(6);
            self.hires.add_css_class("caption-heading");
            self.hires.add_css_class("accent");
            self.duration.add_css_class("numeric");
            self.duration.add_css_class("dim-label");

            row.append(&self.cover);
            row.append(&columns);
            row.append(&self.hires);
            row.append(&self.duration);
        }
    }

    impl WidgetImpl for TrackRow {}
    impl BoxImpl for TrackRow {}
}

glib::wrapper! {
    pub struct TrackRow(ObjectSubclass<imp::TrackRow>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl TrackRow {
    pub fn new() -> Self {
        glib::Object::new()
    }

    pub fn bind(&self, track: &TrackInfo, position: u32, covers: &Rc<CoverLoader>) {
        let imp = self.imp();
        imp.title.set_label(&track.title);
        imp.artists.set_label(&track.artist_names());
        imp.album.set_label(&track.album);
        imp.hires.set_label(if track.hires { "HI-RES" } else { "" });
        imp.duration
            .set_label(&format_duration(track.duration.into()));

        imp.cover.set_icon_name(Some(COVER_PLACEHOLDER));
        let ticket = track.cover.as_deref().and_then(|id| {
            let row = self.downgrade();
            let pixels = 48 * row_scale(self);
            covers.load(&Image::Cover(id.to_owned()), pixels, position, move |texture| {
                if let Some(row) = row.upgrade() {
                    row.imp().cover.set_paintable(Some(texture));
                }
            })
        });
        imp.cover_ticket.replace(ticket);
    }

    pub fn unbind(&self) {
        self.imp().cover_ticket.take();
    }
}

fn row_scale(widget: &impl IsA<gtk::Widget>) -> u32 {
    widget.scale_factor().max(1) as u32
}

impl Default for TrackRow {
    fn default() -> Self {
        Self::new()
    }
}
