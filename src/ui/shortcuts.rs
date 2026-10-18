//! The Keyboard Shortcuts dialog (docs/design.md §3.15, §5.2). Items with
//! an action show that action's accelerator, and those of the window's key
//! controller come from its table, so they never go out of sync.

use adw::prelude::*;

use crate::i18n::tr;

/// An item: from an action (`win.…`/`app.…`) with an accelerator, from a
/// window action bound by the key controller ([`super::window::key_accel`]),
/// or a literal accelerator.
enum Item {
    Action(&'static str, &'static str),
    Key(&'static str, &'static str),
    Keys(&'static str, &'static str),
}

/// Marks a constant for translation; [`tr`] translates it where it is
/// shown.
const fn n_(text: &'static str) -> &'static str {
    text
}

const SECTIONS: &[(&str, &[Item])] = &[
    (
        n_("General"),
        &[
            Item::Action(n_("Focus Search"), "win.search"),
            Item::Action(n_("Preferences"), "win.preferences"),
            Item::Action(n_("Keyboard Shortcuts"), "win.shortcuts"),
            Item::Key(n_("Undo"), "undo"),
            Item::Keys(n_("Reload Page"), "F5"),
            Item::Action(n_("Close Window"), "win.close"),
            Item::Action(n_("Quit"), "app.quit"),
        ],
    ),
    (
        n_("Navigation"),
        &[
            Item::Action(n_("Go to Home"), "win.section('home')"),
            Item::Action(n_("Go to Explore"), "win.section('explore')"),
            Item::Action(n_("Go to Search"), "win.section('search')"),
            Item::Action(n_("Go to Collection"), "win.section('collection')"),
            Item::Action(n_("Back"), "win.back"),
            Item::Action(n_("Show Sidebar"), "win.show-sidebar"),
            Item::Keys(n_("Next Region"), "F6"),
            Item::Keys(n_("Previous Region"), "<Shift>F6"),
            Item::Keys(n_("Close Panel or Popover"), "Escape"),
        ],
    ),
    (
        n_("Now Playing"),
        &[
            Item::Action(n_("Now Playing"), "win.panel('playing')"),
            Item::Action(n_("Lyrics"), "win.panel('lyrics')"),
            Item::Action(n_("Queue"), "win.panel('queue')"),
        ],
    ),
    (
        n_("Playback"),
        &[
            Item::Key(n_("Play/Pause"), "play-pause"),
            Item::Action(n_("Play/Pause Anywhere"), "win.play-pause"),
            Item::Key(n_("Next Track"), "next"),
            Item::Key(n_("Previous Track"), "previous"),
            Item::Key(n_("Seek Forward 10 Seconds"), "seek-forward"),
            Item::Key(n_("Seek Backward 10 Seconds"), "seek-backward"),
            Item::Key(n_("Volume Up"), "volume-up"),
            Item::Key(n_("Volume Down"), "volume-down"),
            Item::Key(n_("Mute"), "mute"),
            Item::Key(n_("Shuffle"), "shuffle"),
            Item::Key(n_("Repeat"), "repeat"),
            Item::Key(n_("Toggle Favourite"), "favorite"),
        ],
    ),
    (
        n_("Lists"),
        &[
            Item::Keys(n_("Play or Open"), "Return"),
            Item::Keys(n_("Context Menu"), "<Shift>F10 Menu"),
            Item::Keys(n_("Remove"), "Delete"),
            Item::Keys(n_("Move Up"), "<Alt>Up"),
            Item::Keys(n_("Move Down"), "<Alt>Down"),
        ],
    ),
];

/// libadwaita 1.8's shortcuts dialog.
#[cfg(feature = "adw-1-8")]
pub fn present(window: &adw::ApplicationWindow) {
    let dialog = adw::ShortcutsDialog::new();
    for (title, items) in SECTIONS {
        let section = adw::ShortcutsSection::new(Some(tr(title)));
        for item in *items {
            let item = match item {
                Item::Action(title, action) => adw::ShortcutsItem::from_action(tr(title), action),
                Item::Key(title, action) => match super::window::key_accel(action) {
                    Some(accel) => adw::ShortcutsItem::new(tr(title), &accel),
                    None => continue,
                },
                Item::Keys(title, keys) => adw::ShortcutsItem::new(tr(title), keys),
            };
            section.add(item);
        }
        dialog.add(section);
    }
    dialog.present(Some(window));
}

/// Without libadwaita 1.8 (GTK 4.18 + libadwaita 1.7 systems): the same
/// sections as rows, each with its keys as text.
#[cfg(not(feature = "adw-1-8"))]
pub fn present(window: &adw::ApplicationWindow) {
    let page = adw::PreferencesPage::new();
    for (title, items) in SECTIONS {
        let group = adw::PreferencesGroup::builder().title(tr(title)).build();
        for item in *items {
            let (title, accels) = match item {
                Item::Action(title, action) => {
                    let Some(app) = window.application() else {
                        continue;
                    };
                    let accels = app.accels_for_action(action);
                    let accels: Vec<&str> = accels.iter().map(|accel| accel.as_str()).collect();
                    (title, accels.join(" "))
                }
                Item::Key(title, action) => match super::window::key_accel(action) {
                    Some(accel) => (title, accel),
                    None => continue,
                },
                Item::Keys(title, keys) => (title, (*keys).to_owned()),
            };
            let keys = accels
                .split_whitespace()
                .filter_map(|accel| {
                    let (key, modifiers) = gtk::accelerator_parse(accel)?;
                    Some(gtk::accelerator_get_label(key, modifiers).to_string())
                })
                .collect::<Vec<_>>();
            if keys.is_empty() {
                continue;
            }
            let label = gtk::Label::builder()
                .label(keys.join(" / "))
                .valign(gtk::Align::Center)
                .css_classes(["dim-label"])
                .build();
            let row = adw::ActionRow::builder().title(tr(title)).build();
            row.add_suffix(&label);
            group.add(&row);
        }
        page.add(&group);
    }
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&page));
    let dialog = adw::Dialog::builder()
        .title(tr("Keyboard Shortcuts"))
        .content_width(480)
        .content_height(640)
        .child(&toolbar)
        .build();
    dialog.present(Some(window));
}
