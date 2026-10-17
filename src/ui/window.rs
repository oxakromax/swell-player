//! The main window: a stack that switches between loading, sign-in and the
//! library once a session exists.

use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use gtk::glib;
use tidalrs::AudioQuality;

use crate::api::{self, Api};
use crate::{config, runtime, session};

pub fn build(app: &adw::Application) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(config::APP_NAME)
        .default_width(960)
        .default_height(720)
        .build();

    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .build();
    let spinner = adw::Spinner::builder()
        .width_request(32)
        .height_request(32)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    stack.add_named(&spinner, Some("loading"));

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&stack));
    window.set_content(Some(&toolbar));
    window.present();

    let ui = Rc::new(Ui { window, stack, http: api::http_client() });
    glib::spawn_future_local(async move {
        match runtime::spawn(session::load()).await {
            Ok(Some(stored)) => ui.show_library(Api::restore(stored, ui.http.clone())),
            Ok(None) => ui.show_login(None),
            Err(err) => ui.show_login(Some(format!("Could not open the keyring: {err:#}"))),
        }
    });
}

struct Ui {
    window: adw::ApplicationWindow,
    stack: gtk::Stack,
    http: reqwest::Client,
}

impl Ui {
    fn set_page(&self, name: &str, page: &impl IsA<gtk::Widget>) {
        if let Some(old) = self.stack.child_by_name(name) {
            self.stack.remove(&old);
        }
        self.stack.add_named(page, Some(name));
        self.stack.set_visible_child_name(name);
    }

    fn show_login(self: &Rc<Self>, error: Option<String>) {
        let ui = self.clone();
        let page = super::login::page(&self.window, self.http.clone(), error, move |api| {
            ui.show_library(api);
        });
        self.set_page("login", &page);
    }

    fn show_library(self: &Rc<Self>, api: Arc<Api>) {
        let quality = match api.flow.max_quality() {
            AudioQuality::HiResLossless => "Hi-Res Lossless",
            AudioQuality::Lossless => "Lossless",
            AudioQuality::High | AudioQuality::Low => "AAC",
        };
        let page = adw::StatusPage::builder()
            .icon_name("emblem-ok-symbolic")
            .title("Signed in")
            .description(format!("Best quality: {quality}"))
            .build();
        let sign_out = gtk::Button::builder()
            .label("Sign Out")
            .css_classes(["pill"])
            .halign(gtk::Align::Center)
            .build();
        page.set_child(Some(&sign_out));

        sign_out.connect_clicked({
            let ui = self.clone();
            let api = api.clone();
            move |_| {
                let ui = ui.clone();
                let api = api.clone();
                glib::spawn_future_local(async move {
                    if let Err(err) = runtime::spawn(async move { api.sign_out().await }).await {
                        log::warn!("could not remove the stored session: {err:#}");
                    }
                    ui.show_login(None);
                });
            }
        });

        glib::spawn_future_local({
            let page = page.clone();
            async move {
                let description = match runtime::spawn(async move { api.favorite_track_count().await }).await {
                    Ok(count) => format!("Best quality: {quality} · {count} favourite tracks"),
                    Err(err) => format!("Best quality: {quality} · could not reach TIDAL: {err:#}"),
                };
                page.set_description(Some(&description));
            }
        });

        self.set_page("library", &page);
    }
}
