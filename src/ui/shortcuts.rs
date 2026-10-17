//! The Keyboard Shortcuts dialog (docs/design.md §3.15, §5.2). Items with
//! an action show that action's accelerator, and those of the window's key
//! controller come from its table, so they never go out of sync.

use adw::prelude::*;

/// An item: from an action (`win.…`/`app.…`) with an accelerator, from a
/// window action bound by the key controller ([`super::window::key_accel`]),
/// or a literal accelerator.
enum Item {
    Action(&'static str, &'static str),
    Key(&'static str, &'static str),
    Keys(&'static str, &'static str),
}

const SECTIONS: &[(&str, &[Item])] = &[
    (
        "General",
        &[
            Item::Action("Focus Search", "win.search"),
            Item::Action("Preferences", "win.preferences"),
            Item::Action("Keyboard Shortcuts", "win.shortcuts"),
            Item::Key("Undo", "undo"),
            Item::Keys("Reload Page", "F5"),
            Item::Action("Close Window", "win.close"),
            Item::Action("Quit", "app.quit"),
        ],
    ),
    (
        "Navigation",
        &[
            Item::Action("Go to Home", "win.section('home')"),
            Item::Action("Go to Explore", "win.section('explore')"),
            Item::Action("Go to Search", "win.section('search')"),
            Item::Action("Go to Collection", "win.section('collection')"),
            Item::Action("Back", "win.back"),
            Item::Action("Show Sidebar", "win.show-sidebar"),
            Item::Keys("Next Region", "F6"),
            Item::Keys("Previous Region", "<Shift>F6"),
            Item::Keys("Close Panel or Popover", "Escape"),
        ],
    ),
    (
        "Now Playing",
        &[
            Item::Action("Now Playing", "win.panel('playing')"),
            Item::Action("Lyrics", "win.panel('lyrics')"),
            Item::Action("Queue", "win.panel('queue')"),
        ],
    ),
    (
        "Playback",
        &[
            Item::Key("Play/Pause", "play-pause"),
            Item::Action("Play/Pause Anywhere", "win.play-pause"),
            Item::Key("Next Track", "next"),
            Item::Key("Previous Track", "previous"),
            Item::Key("Seek Forward 10 Seconds", "seek-forward"),
            Item::Key("Seek Backward 10 Seconds", "seek-backward"),
            Item::Key("Volume Up", "volume-up"),
            Item::Key("Volume Down", "volume-down"),
            Item::Key("Mute", "mute"),
            Item::Key("Shuffle", "shuffle"),
            Item::Key("Repeat", "repeat"),
            Item::Key("Toggle Favourite", "favorite"),
        ],
    ),
    (
        "Lists",
        &[
            Item::Keys("Play or Open", "Return"),
            Item::Keys("Context Menu", "<Shift>F10 Menu"),
            Item::Keys("Remove", "Delete"),
            Item::Keys("Move Up", "<Alt>Up"),
            Item::Keys("Move Down", "<Alt>Down"),
        ],
    ),
];

pub fn present(window: &adw::ApplicationWindow) {
    let dialog = adw::ShortcutsDialog::new();
    for (title, items) in SECTIONS {
        let section = adw::ShortcutsSection::new(Some(title));
        for item in *items {
            let item = match item {
                Item::Action(title, action) => adw::ShortcutsItem::from_action(title, action),
                Item::Key(title, action) => match super::window::key_accel(action) {
                    Some(accel) => adw::ShortcutsItem::new(title, &accel),
                    None => continue,
                },
                Item::Keys(title, keys) => adw::ShortcutsItem::new(title, keys),
            };
            section.add(item);
        }
        dialog.add(section);
    }
    dialog.present(Some(window));
}
