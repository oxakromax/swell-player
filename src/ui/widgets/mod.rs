//! Reusable widgets (docs/design.md §4).

pub mod carousel;
pub mod heart_button;
pub mod lyrics_view;
pub mod media_card;
pub mod mini_player;
pub mod now_playing;
mod np_common;
mod np_controls;
pub mod page_header;
pub mod player_bar;
pub mod quality_badge;
pub mod queue_view;
pub mod section;
pub mod skeleton;
pub mod status;
pub mod track_list;
pub mod track_row;

use gtk::prelude::*;
use gtk::gdk;

/// `preferred` if the icon theme has it, `fallback` otherwise. The `swell-*`
/// icons (docs/design.md §1.9) come from the app's resources; until they are
/// installed, a standard icon stands in.
pub fn icon_name(preferred: &'static str, fallback: &'static str) -> &'static str {
    let Some(display) = gdk::Display::default() else {
        return fallback;
    };
    if gtk::IconTheme::for_display(&display).has_icon(preferred) {
        preferred
    } else {
        fallback
    }
}

/// Device pixels for `logical` pixels on `widget`'s surface.
pub fn device_pixels(widget: &impl IsA<gtk::Widget>, logical: i32) -> u32 {
    (logical.max(1) * widget.scale_factor().max(1)) as u32
}

/// Sets the accessible name of an icon-only button and its tooltip.
pub fn label_button(button: &impl IsA<gtk::Widget>, label: &str) {
    button.set_tooltip_text(Some(label));
    button
        .upcast_ref::<gtk::Widget>()
        .update_property(&[gtk::accessible::Property::Label(label)]);
}
