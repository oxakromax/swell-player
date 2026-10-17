use std::rc::Rc;

use super::super::context::Ctx;
use crate::tidal::pages::More;

pub fn page(_ctx: &Rc<Ctx>, title: String, _more: More) -> adw::NavigationPage {
    super::placeholder(&title)
}
