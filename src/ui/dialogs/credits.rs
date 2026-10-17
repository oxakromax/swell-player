//! Credits (a track's, or an album's from its page) and the "Read More"
//! text dialog for reviews and biographies.

use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use crate::model::TrackInfo;
use crate::runtime;
use crate::tidal::models::Credit;
use crate::ui::context::Ctx;
use crate::ui::nav::Route;

/// The credits of a track, loaded when the dialog opens.
pub fn show(ctx: &Rc<Ctx>, track: &TrackInfo) {
    let stack = gtk::Stack::new();
    let spinner = adw::Spinner::builder()
        .width_request(32)
        .height_request(32)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    stack.add_named(&spinner, Some("loading"));
    let dialog = dialog("Credits", &track.title, &stack);
    dialog.present(Some(&ctx.window));

    let id = track.id;
    let api = ctx.api.clone();
    let weak_ctx = Rc::downgrade(ctx);
    let weak_dialog = dialog.downgrade();
    let task = glib::spawn_future_local(async move {
        let result = runtime::spawn(async move { api.track_credits(id).await }).await;
        let (Some(ctx), Some(dialog)) = (weak_ctx.upgrade(), weak_dialog.upgrade()) else {
            return;
        };
        let content: gtk::Widget = match result {
            Ok(credits) if credits.iter().any(|c| !c.contributors.is_empty()) => {
                let dialog = dialog.downgrade();
                credits_page(&ctx, &credits, move || {
                    if let Some(dialog) = dialog.upgrade() {
                        dialog.close();
                    }
                })
                .upcast()
            }
            Ok(_) => super::super::widgets::status::empty_status(
                "avatar-default-symbolic",
                "No Credits",
                "TIDAL has no credits for this track.",
            )
            .upcast(),
            Err(err) => {
                if crate::tidal::is_auth_error(&err) {
                    ctx.report("Couldn't load the credits", &err);
                }
                super::super::widgets::status::empty_status(
                    "network-error-symbolic",
                    "Couldn't Load the Credits",
                    &super::super::widgets::status::describe_error(&err),
                )
                .upcast()
            }
        };
        stack.add_named(&content, Some("content"));
        stack.set_visible_child_name("content");
    });
    dialog.connect_closed(move |_| task.abort());
}

/// One group per role, one row per contributor; contributors with an id
/// open their artist page (after `on_navigate`, e.g. closing a dialog).
pub fn credits_page(
    ctx: &Rc<Ctx>,
    credits: &[Credit],
    on_navigate: impl Fn() + 'static,
) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let on_navigate = Rc::new(on_navigate);
    for credit in credits.iter().filter(|c| !c.contributors.is_empty()) {
        let group = adw::PreferencesGroup::builder()
            .title(glib::markup_escape_text(&credit.role).as_str())
            .build();
        for contributor in &credit.contributors {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&contributor.name).as_str())
                .build();
            if let Some(id) = contributor.id.filter(|&id| id != 0) {
                row.set_activatable(true);
                row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
                let ctx = Rc::downgrade(ctx);
                let on_navigate = on_navigate.clone();
                row.connect_activated(move |_| {
                    on_navigate();
                    if let Some(ctx) = ctx.upgrade() {
                        ctx.navigate(Route::Artist(id));
                    }
                });
            }
            group.add(&row);
        }
        page.add(&group);
    }
    page
}

/// A review or biography in full, selectable.
pub fn show_text(ctx: &Ctx, title: &str, subtitle: &str, text: &str) {
    let label = gtk::Label::builder()
        .label(text)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .xalign(0.0)
        .yalign(0.0)
        .selectable(true)
        .margin_top(12)
        .margin_bottom(24)
        .margin_start(24)
        .margin_end(24)
        .build();
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .child(&label)
        .build();
    dialog(title, subtitle, &scrolled).present(Some(&ctx.window));
}

fn dialog(title: &str, subtitle: &str, content: &impl IsA<gtk::Widget>) -> adw::Dialog {
    let header = adw::HeaderBar::builder()
        .title_widget(&adw::WindowTitle::new(title, subtitle))
        .build();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(content));
    adw::Dialog::builder()
        .title(title)
        .content_width(480)
        .content_height(560)
        .child(&toolbar)
        .build()
}
