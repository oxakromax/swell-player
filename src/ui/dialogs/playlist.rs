//! Editing the user's playlists: "Add to Playlist" (direct and through a
//! chooser), New Playlist, Edit Details, Make Public/Private and Delete.
//!
//! Every change reloads the library's playlist list (sidebar, menus) and
//! forgets the cached copy of the playlist.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::covers::{Image, Ticket};
use crate::runtime;
use crate::tidal::Base;
use crate::tidal::models::Playlist;
use crate::ui::context::Ctx;
use crate::ui::menus::toast_with_action;
use crate::ui::nav::Route;

/// Called after any change to playlist `uuid`.
fn changed(ctx: &Rc<Ctx>, uuid: &str) {
    ctx.library.refresh_playlists();
    let api = ctx.api.clone();
    let prefix = format!("playlists/{uuid}");
    glib::spawn_future_local(async move {
        let _ = runtime::spawn(async move {
            api.invalidate(Base::V1, &prefix).await;
            Ok(())
        })
        .await;
    });
}

fn open_button(ctx: &Rc<Ctx>, uuid: String) -> impl Fn() + 'static {
    let ctx = Rc::downgrade(ctx);
    move || {
        if let Some(ctx) = ctx.upgrade() {
            ctx.navigate(Route::Playlist(uuid.clone()));
        }
    }
}

/// Adds tracks to an own playlist, with a toast that opens it.
pub fn add_to_playlist(ctx: &Rc<Ctx>, playlist: &Playlist, track_ids: Vec<u64>) {
    if track_ids.is_empty() {
        return;
    }
    let ctx = ctx.clone();
    let uuid = playlist.uuid.clone();
    let title = playlist.title.clone();
    glib::spawn_future_local(async move {
        let api = ctx.api.clone();
        let count = track_ids.len();
        let request = uuid.clone();
        match runtime::spawn(async move { api.add_to_playlist(&request, &track_ids).await }).await {
            Ok(()) => {
                let message = if count == 1 {
                    format!("Added to “{title}”")
                } else {
                    format!("Added {count} tracks to “{title}”")
                };
                toast_with_action(&ctx, &message, "Open", open_button(&ctx, uuid.clone()));
                changed(&ctx, &uuid);
            }
            Err(err) => ctx.report(
                &format!("Couldn't add to “{}”", glib::markup_escape_text(&title)),
                &err,
            ),
        }
    });
}

/// The "Add to Playlist" chooser: search, own playlists, "New Playlist…".
pub fn choose_playlist(ctx: &Rc<Ctx>, track_ids: Vec<u64>) {
    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    for playlist in ctx.library.own_playlists() {
        store.append(&glib::BoxedAnyObject::new(playlist));
    }
    let search = gtk::SearchEntry::builder()
        .placeholder_text("Search Playlists")
        .hexpand(true)
        .build();
    let filter = gtk::CustomFilter::new({
        let search = search.downgrade();
        move |object| {
            let Some(search) = search.upgrade() else {
                return true;
            };
            let query = search.text().to_lowercase();
            query.is_empty()
                || object
                    .downcast_ref::<glib::BoxedAnyObject>()
                    .is_some_and(|o| o.borrow::<Playlist>().title.to_lowercase().contains(&query))
        }
    });
    search.connect_search_changed({
        let filter = filter.downgrade();
        move |_| {
            if let Some(filter) = filter.upgrade() {
                filter.changed(gtk::FilterChange::Different);
            }
        }
    });
    let model = gtk::FilterListModel::new(Some(store), Some(filter));

    let tickets: Rc<RefCell<HashMap<gtk::Widget, Ticket>>> = Rc::default();
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
        let row = gtk::Box::builder()
            .spacing(12)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(6)
            .margin_end(6)
            .build();
        let cover = gtk::Image::builder()
            .pixel_size(32)
            .icon_name("view-list-symbolic")
            .overflow(gtk::Overflow::Hidden)
            .css_classes(["cover"])
            .build();
        let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let title = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        let count = gtk::Label::builder()
            .xalign(0.0)
            .css_classes(["caption", "dimmed"])
            .build();
        text.append(&title);
        text.append(&count);
        row.append(&cover);
        row.append(&text);
        item.set_child(Some(&row));
    });
    factory.connect_bind({
        let ctx = Rc::downgrade(ctx);
        let tickets = tickets.clone();
        move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
            let (Some(row), Some(object), Some(ctx)) = (
                item.child(),
                item.item().and_downcast::<glib::BoxedAnyObject>(),
                ctx.upgrade(),
            ) else {
                return;
            };
            let playlist = object.borrow::<Playlist>();
            let cover = row.first_child().and_downcast::<gtk::Image>();
            let text = row.last_child();
            if let Some(text) = text {
                if let Some(title) = text.first_child().and_downcast::<gtk::Label>() {
                    title.set_label(&playlist.title);
                }
                if let Some(count) = text.last_child().and_downcast::<gtk::Label>() {
                    count.set_label(&match playlist.number_of_tracks {
                        1 => "1 track".to_owned(),
                        n => format!("{n} tracks"),
                    });
                }
            }
            item.set_accessible_label(&playlist.title);
            if let (Some(cover), Some(image)) = (cover, playlist.square_image.clone()) {
                cover.set_icon_name(Some("view-list-symbolic"));
                let weak = cover.downgrade();
                let pixels = super::super::widgets::device_pixels(&cover, 32);
                if let Some(ticket) = ctx.covers.load(
                    &Image::Playlist(image),
                    pixels,
                    item.position(),
                    move |texture| {
                        if let Some(cover) = weak.upgrade() {
                            cover.set_paintable(Some(texture));
                        }
                    },
                ) {
                    tickets.borrow_mut().insert(row.clone(), ticket);
                }
            }
        }
    });
    factory.connect_unbind({
        let tickets = tickets.clone();
        move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
            if let Some(row) = item.child() {
                tickets.borrow_mut().remove(&row);
            }
        }
    });
    let view = gtk::ListView::builder()
        .model(&gtk::NoSelection::new(Some(model.clone())))
        .factory(&factory)
        .single_click_activate(true)
        .css_classes(["navigation-sidebar"])
        .build();
    view.update_property(&[gtk::accessible::Property::Label("Your Playlists")]);
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&view)
        .build();

    let new = gtk::Button::builder()
        .child(
            &adw::ButtonContent::builder()
                .icon_name("list-add-symbolic")
                .label("New Playlist…")
                .halign(gtk::Align::Start)
                .build(),
        )
        .css_classes(["flat"])
        .margin_start(6)
        .margin_end(6)
        .build();

    let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    content.append(&new);
    content.append(&scrolled);
    let search_bar = gtk::Box::builder()
        .margin_start(12)
        .margin_end(12)
        .margin_bottom(6)
        .build();
    search_bar.append(&search);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.add_top_bar(&search_bar);
    toolbar.set_content(Some(&content));
    let dialog = adw::Dialog::builder()
        .title("Add to Playlist")
        .content_width(400)
        .content_height(520)
        .child(&toolbar)
        .build();
    dialog.set_focus(Some(&search));

    let ids = Rc::new(track_ids);
    view.connect_activate({
        let ctx = Rc::downgrade(ctx);
        let dialog = dialog.downgrade();
        let ids = ids.clone();
        move |_, position| {
            let (Some(ctx), Some(object)) = (
                ctx.upgrade(),
                model.item(position).and_downcast::<glib::BoxedAnyObject>(),
            ) else {
                return;
            };
            add_to_playlist(&ctx, &object.borrow::<Playlist>(), (*ids).clone());
            if let Some(dialog) = dialog.upgrade() {
                dialog.close();
            }
        }
    });
    new.connect_clicked({
        let ctx = Rc::downgrade(ctx);
        let dialog = dialog.downgrade();
        move |_| {
            if let Some(dialog) = dialog.upgrade() {
                dialog.close();
            }
            if let Some(ctx) = ctx.upgrade() {
                new_playlist(&ctx, (*ids).clone());
            }
        }
    });
    dialog.present(Some(&ctx.window));
}

/// The form shared by "New Playlist" and "Edit Details".
struct Form {
    dialog: adw::Dialog,
    title: adw::EntryRow,
    description: adw::EntryRow,
    public: adw::SwitchRow,
    confirm: gtk::Button,
}

fn form(heading: &str, confirm_label: &str, playlist: Option<&Playlist>) -> Form {
    let title = adw::EntryRow::builder().title("Title").build();
    let description = adw::EntryRow::builder().title("Description").build();
    let public = adw::SwitchRow::builder()
        .title("Public")
        .subtitle("Anyone can find it and see what's in it")
        .build();
    if let Some(playlist) = playlist {
        title.set_text(&playlist.title);
        description.set_text(&playlist.description);
        public.set_active(playlist.public_playlist);
    }
    let group = adw::PreferencesGroup::new();
    group.add(&title);
    group.add(&description);
    group.add(&public);
    let page = adw::PreferencesPage::new();
    page.add(&group);

    let cancel = gtk::Button::with_label("Cancel");
    let confirm = gtk::Button::builder()
        .label(confirm_label)
        .css_classes(["suggested-action"])
        .sensitive(playlist.is_some())
        .build();
    let header = adw::HeaderBar::builder()
        .show_start_title_buttons(false)
        .show_end_title_buttons(false)
        .build();
    header.pack_start(&cancel);
    header.pack_end(&confirm);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&page));
    let dialog = adw::Dialog::builder()
        .title(heading)
        .content_width(420)
        .child(&toolbar)
        .default_widget(&confirm)
        .focus_widget(&title)
        .build();
    title.set_activates_default(true);
    title.connect_changed({
        let confirm = confirm.downgrade();
        move |entry| {
            if let Some(confirm) = confirm.upgrade() {
                confirm.set_sensitive(!entry.text().trim().is_empty());
            }
        }
    });
    cancel.connect_clicked({
        let dialog = dialog.downgrade();
        move |_| {
            if let Some(dialog) = dialog.upgrade() {
                dialog.close();
            }
        }
    });
    Form {
        dialog,
        title,
        description,
        public,
        confirm,
    }
}

/// "New Playlist": creates it, then adds `track_ids` (may be empty).
pub fn new_playlist(ctx: &Rc<Ctx>, track_ids: Vec<u64>) {
    let form = form("New Playlist", "Create", None);
    let Form {
        dialog,
        title,
        description,
        public,
        confirm,
    } = form;
    let ids = Rc::new(track_ids);
    confirm.connect_clicked({
        let ctx = Rc::downgrade(ctx);
        let dialog = dialog.downgrade();
        move |confirm| {
            let (Some(ctx), Some(dialog)) = (ctx.upgrade(), dialog.upgrade()) else {
                return;
            };
            let name = title.text().trim().to_owned();
            let about = description.text().trim().to_owned();
            let public = public.is_active();
            let ids = (*ids).clone();
            confirm.set_sensitive(false);
            let confirm = confirm.downgrade();
            glib::spawn_future_local(async move {
                let api = ctx.api.clone();
                let request = name.clone();
                let result = runtime::spawn(async move {
                    let playlist = api.create_playlist(&request, &about).await?;
                    if public {
                        api.set_playlist_public(&playlist.uuid, true).await?;
                    }
                    if !ids.is_empty() {
                        api.add_to_playlist(&playlist.uuid, &ids).await?;
                    }
                    Ok(playlist.uuid)
                })
                .await;
                match result {
                    Ok(uuid) => {
                        dialog.close();
                        toast_with_action(
                            &ctx,
                            &format!("Created “{name}”"),
                            "Open",
                            open_button(&ctx, uuid.clone()),
                        );
                        changed(&ctx, &uuid);
                    }
                    Err(err) => {
                        if let Some(confirm) = confirm.upgrade() {
                            confirm.set_sensitive(true);
                        }
                        ctx.report("Couldn't create the playlist", &err);
                    }
                }
            });
        }
    });
    dialog.present(Some(&ctx.window));
}

/// "Edit Details…": title, description and visibility of an own playlist.
pub fn edit_playlist(ctx: &Rc<Ctx>, playlist: &Playlist) {
    let Form {
        dialog,
        title,
        description,
        public,
        confirm,
    } = form("Edit Details", "Save", Some(playlist));
    let original = playlist.clone();
    confirm.connect_clicked({
        let ctx = Rc::downgrade(ctx);
        let dialog = dialog.downgrade();
        move |confirm| {
            let (Some(ctx), Some(dialog)) = (ctx.upgrade(), dialog.upgrade()) else {
                return;
            };
            let name = title.text().trim().to_owned();
            let about = description.text().trim().to_owned();
            let public = public.is_active();
            let uuid = original.uuid.clone();
            let details_changed = name != original.title || about != original.description;
            let visibility_changed = public != original.public_playlist;
            if !details_changed && !visibility_changed {
                dialog.close();
                return;
            }
            confirm.set_sensitive(false);
            let confirm = confirm.downgrade();
            glib::spawn_future_local(async move {
                let api = ctx.api.clone();
                let request = uuid.clone();
                let result = runtime::spawn(async move {
                    if details_changed {
                        api.edit_playlist(&request, &name, &about).await?;
                    }
                    if visibility_changed {
                        api.set_playlist_public(&request, public).await?;
                    }
                    Ok(())
                })
                .await;
                match result {
                    Ok(()) => {
                        dialog.close();
                        ctx.toast("Playlist updated");
                        changed(&ctx, &uuid);
                    }
                    Err(err) => {
                        if let Some(confirm) = confirm.upgrade() {
                            confirm.set_sensitive(true);
                        }
                        ctx.report("Couldn't update the playlist", &err);
                    }
                }
            });
        }
    });
    dialog.present(Some(&ctx.window));
}

/// "Make Public" / "Make Private".
pub fn set_public(ctx: &Rc<Ctx>, playlist: &Playlist, public: bool) {
    let ctx = ctx.clone();
    let uuid = playlist.uuid.clone();
    glib::spawn_future_local(async move {
        let api = ctx.api.clone();
        let request = uuid.clone();
        match runtime::spawn(async move { api.set_playlist_public(&request, public).await }).await {
            Ok(()) => {
                ctx.toast(if public {
                    "The playlist is now public"
                } else {
                    "The playlist is now private"
                });
                changed(&ctx, &uuid);
            }
            Err(err) => ctx.report("Couldn't change the playlist's visibility", &err),
        }
    });
}

/// "Delete Playlist…": asks first (it cannot be undone), then deletes and
/// calls `on_deleted`.
pub fn delete_playlist(ctx: &Rc<Ctx>, playlist: &Playlist, on_deleted: impl Fn() + 'static) {
    let alert = adw::AlertDialog::builder()
        .heading(format!("Delete “{}”?", playlist.title))
        .body("The playlist will be deleted from your TIDAL account. This can't be undone.")
        .default_response("cancel")
        .close_response("cancel")
        .build();
    alert.add_responses(&[("cancel", "Cancel"), ("delete", "Delete")]);
    alert.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    let ctx_weak = Rc::downgrade(ctx);
    let uuid = playlist.uuid.clone();
    let title = playlist.title.clone();
    let on_deleted = Rc::new(on_deleted);
    alert.connect_response(Some("delete"), move |_, _| {
        let Some(ctx) = ctx_weak.upgrade() else {
            return;
        };
        let uuid = uuid.clone();
        let title = title.clone();
        let api = ctx.api.clone();
        let request = uuid.clone();
        let on_deleted = on_deleted.clone();
        glib::spawn_future_local(async move {
            match runtime::spawn(async move { api.delete_playlist(&request).await }).await {
                Ok(()) => {
                    ctx.toast(&format!("Deleted “{}”", glib::markup_escape_text(&title)));
                    changed(&ctx, &uuid);
                    on_deleted();
                }
                Err(err) => ctx.report("Couldn't delete the playlist", &err),
            }
        });
    });
    alert.present(Some(&ctx.window));
}
