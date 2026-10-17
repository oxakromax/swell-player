//! The now-playing panel: Playing, Lyrics and Queue tabs (docs/design.md
//! §3.13). PROVISIONAL stub; see the brief.

use std::rc::Rc;

use super::super::context::{Ctx, PanelTab};

pub struct NowPlayingPanel {
    pub widget: gtk::Widget,
}

impl NowPlayingPanel {
    pub fn new(_ctx: &Rc<Ctx>) -> Rc<Self> {
        let status = adw::StatusPage::builder()
            .icon_name("audio-x-generic-symbolic")
            .title("Now Playing")
            .build();
        Rc::new(Self {
            widget: status.into(),
        })
    }

    /// Shows `tab` (the shell has already made the panel visible).
    pub fn show_tab(&self, _tab: PanelTab) {}
}
