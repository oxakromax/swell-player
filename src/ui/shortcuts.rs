//! The Keyboard Shortcuts dialog (docs/design.md §3.15, §5.2). Items with
//! an action show that action's accelerator, so they never go out of sync.

use adw::prelude::*;

/// An item: from an action (`win.…`/`app.…`) or a literal accelerator.
enum Item {
    Action(&'static str, &'static str),
    Keys(&'static str, &'static str),
}

const SECTIONS: &[(&str, &[Item])] = &[
    (
        "General",
        &[
            Item::Action("Search", "win.search"),
            Item::Action("Preferences", "win.preferences"),
            Item::Action("Keyboard Shortcuts", "win.shortcuts"),
            Item::Action("Undo", "win.undo"),
            Item::Action("Close Window", "win.close"),
            Item::Action("Quit", "app.quit"),
        ],
    ),
    (
        "Navigation",
        &[
            Item::Action("Home", "win.section('home')"),
            Item::Action("Explore", "win.section('explore')"),
            Item::Action("Search", "win.section('search')"),
            Item::Action("Collection", "win.section('collection')"),
            Item::Action("Back", "win.back"),
            Item::Action("Show Sidebar", "win.show-sidebar"),
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
            Item::Keys("Play/Pause", "space"),
            Item::Action("Play/Pause Anywhere", "win.play-pause"),
            Item::Keys("Next Track", "<Control>Right"),
            Item::Keys("Previous Track", "<Control>Left"),
            Item::Keys("Seek Forward 10 Seconds", "<Shift>Right"),
            Item::Keys("Seek Backward 10 Seconds", "<Shift>Left"),
            Item::Keys("Volume Up", "<Control>Up"),
            Item::Keys("Volume Down", "<Control>Down"),
            Item::Action("Mute", "win.mute"),
            Item::Action("Shuffle", "win.shuffle"),
            Item::Action("Repeat", "win.repeat"),
            Item::Action("Add to Favourites", "win.favorite"),
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
            section.add(match item {
                Item::Action(title, action) => adw::ShortcutsItem::from_action(title, action),
                Item::Keys(title, keys) => adw::ShortcutsItem::new(title, keys),
            });
        }
        dialog.add(section);
    }
    dialog.present(Some(window));
}
