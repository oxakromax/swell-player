use std::rc::Rc;

use super::super::context::Ctx;

/// Explore, or the editorial page `path` with its `title`.
pub fn page(_ctx: &Rc<Ctx>, _page: Option<(String, String)>) -> adw::NavigationPage {
    super::placeholder("Explore")
}
