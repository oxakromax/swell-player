use std::rc::Rc;

use super::super::context::Ctx;

pub enum Kind {
    Mix(String),
    TrackRadio(u64),
    ArtistRadio(u64),
}

pub fn page(_ctx: &Rc<Ctx>, _kind: Kind) -> adw::NavigationPage {
    super::placeholder("Mix")
}
