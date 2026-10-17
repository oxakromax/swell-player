use std::rc::Rc;

use super::super::context::Ctx;

pub fn page(_ctx: &Rc<Ctx>, _id: u64) -> adw::NavigationPage {
    super::placeholder("Album")
}

pub fn credits_page(_ctx: &Rc<Ctx>, _id: u64) -> adw::NavigationPage {
    super::placeholder("Credits")
}
