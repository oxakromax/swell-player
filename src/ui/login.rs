//! The sign-in page: browser login (PKCE, Hi-Res) or device code (Lossless).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::api::Api;
use crate::runtime;
use crate::session::LoginFlow;

pub fn page(
    window: &adw::ApplicationWindow,
    http: reqwest::Client,
    error: Option<String>,
    on_signed_in: impl Fn(Arc<Api>) + 'static,
) -> adw::StatusPage {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .halign(gtk::Align::Center)
        .width_request(360)
        .build();

    let browser_button = pill_button("Sign in with Browser", true);
    let browser_hint = hint("Hi-Res Lossless, up to 24-bit/192 kHz");

    let redirect_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .visible(false)
        .build();
    let redirect_help = gtk::Label::builder()
        .label("After you sign in, TIDAL shows a page that says “Oops”. Copy that page’s address and paste it here.")
        .wrap(true)
        .xalign(0.0)
        .build();
    let redirect_entry = gtk::Entry::builder()
        .placeholder_text("https://tidal.com/android/login/auth?code=…")
        .input_purpose(gtk::InputPurpose::Url)
        .build();
    let continue_button = gtk::Button::builder()
        .label("Continue")
        .css_classes(["suggested-action"])
        .build();
    redirect_box.append(&redirect_help);
    redirect_box.append(&redirect_entry);
    redirect_box.append(&continue_button);

    let device_button = pill_button("Use a Device Code", false);
    let device_hint = hint("Lossless, up to 16-bit/44.1 kHz");

    let device_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .visible(false)
        .build();
    let device_code = gtk::Label::builder()
        .css_classes(["title-1"])
        .selectable(true)
        .build();
    let device_link = gtk::LinkButton::new("https://link.tidal.com");
    let device_wait = gtk::Box::builder()
        .spacing(6)
        .halign(gtk::Align::Center)
        .build();
    device_wait.append(&adw::Spinner::new());
    device_wait.append(&gtk::Label::new(Some("Waiting for approval…")));
    device_box.append(&device_code);
    device_box.append(&device_link);
    device_box.append(&device_wait);

    let error_label = gtk::Label::builder()
        .wrap(true)
        .css_classes(["error"])
        .visible(error.is_some())
        .label(error.unwrap_or_default())
        .build();

    for widget in [
        browser_button.upcast_ref::<gtk::Widget>(),
        browser_hint.upcast_ref(),
        redirect_box.upcast_ref(),
        device_button.upcast_ref(),
        device_hint.upcast_ref(),
        device_box.upcast_ref(),
        error_label.upcast_ref(),
    ] {
        content.append(widget);
    }

    let status = adw::StatusPage::builder()
        .icon_name("avatar-default-symbolic")
        .title("Sign in to TIDAL")
        .description("Swell needs an active TIDAL subscription.")
        .child(&content)
        .build();

    // Both flows may be started; whichever finishes first wins.
    let done = Rc::new(Cell::new(false));
    let on_signed_in = Rc::new(move |api: Arc<Api>| {
        if !done.replace(true) {
            on_signed_in(api);
        }
    });
    let show_error = {
        let error_label = error_label.clone();
        move |message: String| {
            error_label.set_label(&message);
            error_label.set_visible(true);
        }
    };

    let pkce_api: Rc<RefCell<Option<Arc<Api>>>> = Rc::default();

    browser_button.connect_clicked({
        let window = window.clone();
        let http = http.clone();
        let pkce_api = pkce_api.clone();
        let redirect_entry = redirect_entry.clone();
        let show_error = show_error.clone();
        move |_| {
            let api = Api::new(LoginFlow::Pkce, http.clone());
            let url = match api.pkce_login_url() {
                Ok(url) => url,
                Err(err) => return show_error(format!("{err:#}")),
            };
            gtk::UriLauncher::new(&url).launch(Some(&window), gio::Cancellable::NONE, {
                let show_error = show_error.clone();
                move |result| {
                    if let Err(err) = result {
                        show_error(format!(
                            "Could not open the browser ({err}). Open this address yourself:\n{url}"
                        ));
                    }
                }
            });
            pkce_api.replace(Some(api));
            redirect_box.set_visible(true);
            redirect_entry.grab_focus();
        }
    });

    let finish_pkce = {
        let entry = redirect_entry.clone();
        let button = continue_button.clone();
        let on_signed_in = on_signed_in.clone();
        let show_error = show_error.clone();
        move || {
            let Some(api) = pkce_api.borrow().clone() else {
                return;
            };
            let redirect = entry.text().to_string();
            button.set_sensitive(false);
            let button = button.clone();
            let on_signed_in = on_signed_in.clone();
            let show_error = show_error.clone();
            glib::spawn_future_local(async move {
                let task_api = api.clone();
                match runtime::spawn(async move { task_api.finish_pkce_login(&redirect).await })
                    .await
                {
                    Ok(()) => on_signed_in(api),
                    Err(err) => {
                        show_error(format!("Sign-in failed: {err:#}"));
                        button.set_sensitive(true);
                    }
                }
            });
        }
    };
    continue_button.connect_clicked({
        let finish_pkce = finish_pkce.clone();
        move |_| finish_pkce()
    });
    redirect_entry.connect_activate(move |_| finish_pkce());

    device_button.connect_clicked(move |button| {
        button.set_sensitive(false);
        let api = Api::new(LoginFlow::DeviceCode, http.clone());
        let button = button.clone();
        let device_box = device_box.clone();
        let device_code = device_code.clone();
        let device_link = device_link.clone();
        let on_signed_in = on_signed_in.clone();
        let show_error = show_error.clone();
        glib::spawn_future_local(async move {
            let task_api = api.clone();
            let device =
                match runtime::spawn(async move { task_api.start_device_login().await }).await {
                    Ok(device) => device,
                    Err(err) => {
                        show_error(format!("Could not get a device code: {err:#}"));
                        button.set_sensitive(true);
                        return;
                    }
                };
            device_code.set_label(&device.user_code);
            device_link.set_uri(&device.url);
            device_link.set_label(&device.url);
            device_box.set_visible(true);

            let task_api = api.clone();
            match runtime::spawn(async move { task_api.finish_device_login(&device).await }).await {
                Ok(()) => on_signed_in(api),
                Err(err) => {
                    show_error(format!("Device sign-in failed: {err:#}"));
                    device_box.set_visible(false);
                    button.set_sensitive(true);
                }
            }
        });
    });

    status
}

fn pill_button(label: &str, suggested: bool) -> gtk::Button {
    let button = gtk::Button::builder()
        .label(label)
        .css_classes(["pill"])
        .build();
    if suggested {
        button.add_css_class("suggested-action");
    }
    button
}

fn hint(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .css_classes(["dim-label", "caption"])
        .build()
}
