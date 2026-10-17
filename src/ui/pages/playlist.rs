use std::rc::Rc;

use super::super::context::Ctx;

pub fn page(_ctx: &Rc<Ctx>, _uuid: String) -> adw::NavigationPage {
    super::placeholder("Playlist")
}
