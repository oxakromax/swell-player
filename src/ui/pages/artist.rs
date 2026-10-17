use std::rc::Rc;

use super::super::context::Ctx;

pub fn page(_ctx: &Rc<Ctx>, _id: u64) -> adw::NavigationPage {
    super::placeholder("Artist")
}
