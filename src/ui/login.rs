//! The sign-in view (docs/design.md §3.1): a choice between the browser
//! login (PKCE, Hi-Res) and a device code (Lossless), each with its own
//! page; going back cancels the flow in progress.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::api::Api;
use crate::config;
use crate::i18n::tr;
use crate::runtime;
use crate::tidal::Pkce;

type ShowError = Rc<dyn Fn(String)>;

pub fn page(
    window: &adw::ApplicationWindow,
    http: reqwest::Client,
    error: Option<String>,
    on_signed_in: impl Fn(Arc<Api>) + 'static,
) -> adw::NavigationView {
    let view = adw::NavigationView::new();

    // ---- Choice ----
    let browser_button = pill_button(tr("Sign In with Browser"), true);
    let device_button = pill_button(tr("Use a Device Code"), false);
    let choice_error = error_label();
    if let Some(error) = error {
        choice_error.set_label(&error);
        choice_error.set_visible(true);
    }
    let choices = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .build();
    choices.append(&method(
        &browser_button,
        tr("Hi-Res Lossless, up to 24-bit/192 kHz"),
    ));
    choices.append(&method(
        &device_button,
        tr("Lossless, up to 16-bit/44.1 kHz"),
    ));
    choices.append(&choice_error);
    let choice = status_page(
        config::APP_ID,
        tr("Welcome to Swell"),
        tr("Sign in with your TIDAL account."),
        &choices,
    );
    choice.add_css_class("login");
    view.add(&nav_page(tr("Sign In"), "choice", &choice));

    // ---- Browser ----
    let steps = gtk::Label::builder()
        .label(tr("1. Sign in on the TIDAL page that opened in your browser.\n2. TIDAL then shows a page that says “Oops”.\n3. Copy that page’s address and paste it here."))
        .wrap(true)
        .xalign(0.0)
        .build();
    let redirect_entry = adw::EntryRow::builder()
        .title(tr("Paste the address of the “Oops” page"))
        .input_purpose(gtk::InputPurpose::Url)
        .build();
    let entry_list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    entry_list.append(&redirect_entry);
    let continue_button = pill_button(tr("Continue"), true);
    continue_button.set_sensitive(false);
    let reopen_button = pill_button(tr("Open Browser Again"), false);
    let browser_error = error_label();
    let browser_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .build();
    browser_box.append(&steps);
    browser_box.append(&entry_list);
    browser_box.append(&continue_button);
    browser_box.append(&reopen_button);
    browser_box.append(&browser_error);
    let browser = status_page(
        "web-browser-symbolic",
        tr("Sign In with Browser"),
        tr("Your browser opens TIDAL’s sign-in page."),
        &browser_box,
    );
    let browser_page = nav_page(tr("Browser"), "browser", &browser);
    view.add(&browser_page);

    // ---- Device code ----
    let device_code = gtk::Label::builder()
        .css_classes(["title-1", "monospace"])
        .selectable(true)
        .build();
    let copy_button = gtk::Button::builder()
        .icon_name("edit-copy-symbolic")
        .tooltip_text(tr("Copy Code"))
        .valign(gtk::Align::Center)
        .css_classes(["flat", "circular"])
        .build();
    copy_button.update_property(&[gtk::accessible::Property::Label(tr("Copy Code"))]);
    let code_row = gtk::Box::builder()
        .spacing(6)
        .halign(gtk::Align::Center)
        .build();
    code_row.append(&device_code);
    code_row.append(&copy_button);
    let device_link = gtk::LinkButton::with_label("https://link.tidal.com", "link.tidal.com");
    let device_wait = gtk::Box::builder()
        .spacing(6)
        .halign(gtk::Align::Center)
        .build();
    device_wait.append(&adw::Spinner::new());
    device_wait.append(&gtk::Label::new(Some(tr("Waiting for approval…"))));
    let device_error = error_label();
    let new_code_button = pill_button(tr("Get a New Code"), true);
    new_code_button.set_visible(false);
    let device_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .build();
    for widget in [
        code_row.upcast_ref::<gtk::Widget>(),
        device_link.upcast_ref(),
        device_wait.upcast_ref(),
        device_error.upcast_ref(),
        new_code_button.upcast_ref(),
    ] {
        device_box.append(widget);
    }
    let device = status_page(
        "phone-symbolic",
        tr("Use a Device Code"),
        tr("Open link.tidal.com on any device and enter this code."),
        &device_box,
    );
    let device_page = nav_page(tr("Device Code"), "device", &device);
    view.add(&device_page);

    // ---- Signing in ----
    let done = Rc::new(Cell::new(false));
    let on_signed_in = Rc::new(on_signed_in);
    let signed_in = Rc::new({
        let done = done.clone();
        move |api: Arc<Api>, show_error: ShowError| {
            if done.replace(true) {
                return;
            }
            let done = done.clone();
            let on_signed_in = on_signed_in.clone();
            glib::spawn_future_local(async move {
                let task_api = api.clone();
                match runtime::spawn(async move { task_api.store().await }).await {
                    Ok(()) => on_signed_in(api),
                    Err(err) => {
                        done.set(false);
                        log::warn!("could not save the session in the keyring: {err:#}");
                        show_error(crate::session::save_error().to_owned());
                    }
                }
            });
        }
    });

    // ---- Browser flow ----
    // The PKCE verifier belongs to the URL it built, so opening the browser
    // again reopens the same URL instead of invalidating the first one.
    let pkce: Rc<RefCell<Option<Arc<Pkce>>>> = Rc::default();
    let show_browser_error: ShowError = shower(&browser_error);
    let open_browser = Rc::new({
        let window = window.clone();
        let pkce = pkce.clone();
        let show_error = show_browser_error.clone();
        move || {
            let existing = pkce.borrow().as_ref().map(|pending| pending.url.clone());
            let url = match existing {
                Some(url) => url,
                None => match Api::start_pkce() {
                    Ok(pending) => {
                        let url = pending.url.clone();
                        pkce.replace(Some(Arc::new(pending)));
                        url
                    }
                    Err(err) => {
                        log::warn!("could not start the browser sign-in: {err:#}");
                        return show_error(
                            tr("Couldn't start the sign-in. Please try again.").to_owned(),
                        );
                    }
                },
            };
            gtk::UriLauncher::new(&url).launch(Some(&window), gio::Cancellable::NONE, {
                let show_error = show_error.clone();
                move |result| {
                    if let Err(err) = result {
                        show_error(
                            tr("Couldn't open the browser ({error}). Open this address yourself:\n{url}")
                                .replace("{error}", &err.to_string())
                                .replace("{url}", &url),
                        );
                    }
                }
            });
        }
    });
    browser_button.connect_clicked({
        let view = view.clone();
        let open_browser = open_browser.clone();
        let entry = redirect_entry.clone();
        move |_| {
            view.push_by_tag("browser");
            open_browser();
            entry.grab_focus();
        }
    });
    reopen_button.connect_clicked(move |_| open_browser());
    redirect_entry.connect_changed({
        let button = continue_button.clone();
        move |entry| button.set_sensitive(entry.text().contains("code="))
    });
    let finish_pkce = Rc::new({
        let http = http.clone();
        let pkce = pkce.clone();
        let entry = redirect_entry.clone();
        let button = continue_button.clone();
        let signed_in = signed_in.clone();
        let show_error = show_browser_error.clone();
        let error_label = browser_error.clone();
        move || {
            let Some(pending) = pkce.borrow().clone() else {
                return;
            };
            let redirect = entry.text().to_string();
            if !redirect.contains("code=") {
                return;
            }
            error_label.set_visible(false);
            let http = http.clone();
            button.set_sensitive(false);
            let button = button.clone();
            let signed_in = signed_in.clone();
            let show_error = show_error.clone();
            glib::spawn_future_local(async move {
                match runtime::spawn(
                    async move { Api::finish_pkce(http, &pending, &redirect).await },
                )
                .await
                {
                    Ok(api) => signed_in(api, show_error),
                    Err(err) => {
                        log::warn!("browser sign-in failed: {err:#}");
                        show_error(
                            tr("Sign-in failed. {error}")
                                .replace("{error}", &super::widgets::status::describe_error(&err)),
                        );
                        button.set_sensitive(true);
                    }
                }
            });
        }
    });
    continue_button.connect_clicked({
        let finish_pkce = finish_pkce.clone();
        move |_| finish_pkce()
    });
    redirect_entry.connect_entry_activated(move |_| finish_pkce());

    // ---- Device flow ----
    // A code still being polled is abandoned when the user goes back (or
    // the view goes away).
    let device_task: Rc<RefCell<Option<glib::JoinHandle<()>>>> = Rc::default();
    let show_device_error: ShowError = shower(&device_error);
    let start_device = Rc::new({
        let device_task = device_task.clone();
        let http = http.clone();
        let device_code = device_code.clone();
        let device_link = device_link.clone();
        let device_wait = device_wait.clone();
        let device_error = device_error.clone();
        let new_code_button = new_code_button.clone();
        let show_error = show_device_error.clone();
        let signed_in = signed_in.clone();
        move || {
            if let Some(task) = device_task.take() {
                task.abort();
            }
            device_code.set_label("…");
            device_error.set_visible(false);
            new_code_button.set_visible(false);
            device_wait.set_visible(true);
            let http = http.clone();
            let device_code = device_code.clone();
            let device_link = device_link.clone();
            let device_wait = device_wait.clone();
            let new_code_button = new_code_button.clone();
            let show_error = show_error.clone();
            let signed_in = signed_in.clone();
            let task = glib::spawn_future_local(async move {
                let start_http = http.clone();
                let device =
                    match runtime::spawn(async move { Api::start_device_login(&start_http).await })
                        .await
                    {
                        Ok(device) => device,
                        Err(err) => {
                            log::warn!("could not get a device code: {err:#}");
                            show_error(
                                tr("Couldn't get a code. {error}").replace(
                                    "{error}",
                                    &super::widgets::status::describe_error(&err),
                                ),
                            );
                            device_wait.set_visible(false);
                            new_code_button.set_visible(true);
                            return;
                        }
                    };
                device_code.set_label(&device.user_code);
                device_link.set_uri(&device.url);
                device_link.set_label(device.url.trim_start_matches("https://"));
                match runtime::spawn(async move { Api::finish_device_login(http, &device).await })
                    .await
                {
                    Ok(api) => signed_in(api, show_error),
                    Err(err) => {
                        log::warn!("device sign-in failed: {err:#}");
                        show_error(tr("This code expired or was not approved.").to_owned());
                        device_wait.set_visible(false);
                        new_code_button.set_visible(true);
                    }
                }
            });
            device_task.replace(Some(task));
        }
    });
    device_button.connect_clicked({
        let view = view.clone();
        let start_device = start_device.clone();
        move |_| {
            view.push_by_tag("device");
            start_device();
        }
    });
    new_code_button.connect_clicked(move |_| start_device());
    copy_button.connect_clicked({
        let device_code = device_code.clone();
        move |button| button.clipboard().set_text(&device_code.label())
    });

    view.connect_popped({
        let device_task = device_task.clone();
        move |_, page| match page.tag().as_deref() {
            Some("device") => {
                if let Some(task) = device_task.take() {
                    task.abort();
                }
            }
            Some("browser") => {
                pkce.replace(None);
                redirect_entry.set_text("");
                browser_error.set_visible(false);
            }
            _ => {}
        }
    });
    view.connect_destroy(move |_| {
        if let Some(task) = device_task.take() {
            task.abort();
        }
    });
    view
}

fn nav_page(title: &str, tag: &str, content: &impl IsA<gtk::Widget>) -> adw::NavigationPage {
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(content));
    adw::NavigationPage::builder()
        .title(title)
        .tag(tag)
        .child(&toolbar)
        .build()
}

fn status_page(
    icon: &str,
    title: &str,
    description: &str,
    child: &impl IsA<gtk::Widget>,
) -> adw::StatusPage {
    let clamp = adw::Clamp::builder()
        .maximum_size(360)
        .tightening_threshold(360)
        .child(child)
        .build();
    adw::StatusPage::builder()
        .icon_name(icon)
        .title(title)
        .description(description)
        .child(&clamp)
        .build()
}

/// A sign-in method: its button and the quality it allows.
fn method(button: &gtk::Button, quality: &str) -> gtk::Box {
    let hint = gtk::Label::builder()
        .label(quality)
        .css_classes(["caption", "dimmed"])
        .build();
    let method = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();
    method.append(button);
    method.append(&hint);
    method
}

fn pill_button(label: &str, suggested: bool) -> gtk::Button {
    let button = gtk::Button::builder()
        .label(label)
        .use_underline(true)
        .css_classes(["pill"])
        .build();
    if suggested {
        button.add_css_class("suggested-action");
    }
    button
}

fn error_label() -> gtk::Label {
    gtk::Label::builder()
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .justify(gtk::Justification::Center)
        .css_classes(["error"])
        .visible(false)
        .build()
}

fn shower(label: &gtk::Label) -> ShowError {
    let label = label.clone();
    Rc::new(move |message: String| {
        label.set_label(&message);
        label.set_visible(true);
    })
}
