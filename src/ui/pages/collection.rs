use std::rc::Rc;

use super::super::context::Ctx;
use super::super::nav::CollectionView;

pub fn page(_ctx: &Rc<Ctx>, _view: CollectionView) -> adw::NavigationPage {
    super::placeholder("Collection")
}
