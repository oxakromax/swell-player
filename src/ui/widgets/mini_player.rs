//! The narrow-layout mini player (docs/design.md §4.10). PROVISIONAL stub.

use std::rc::Rc;

use super::super::context::Ctx;

pub struct MiniPlayer {
    pub widget: gtk::Widget,
}

impl MiniPlayer {
    pub fn new(_ctx: &Rc<Ctx>) -> Rc<Self> {
        Rc::new(Self {
            widget: gtk::Box::new(gtk::Orientation::Horizontal, 0).into(),
        })
    }
}
