use std::rc::Rc;

use super::super::context::Ctx;

pub fn page(_ctx: &Rc<Ctx>) -> adw::NavigationPage {
    super::placeholder("Recently Played")
}
