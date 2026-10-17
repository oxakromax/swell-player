//! Page states (docs/design.md §3 "Estados comunes", §4.8): a stack with
//! the skeleton, the content, an empty state and an error with "Retry".

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;

/// The skeleton appears only if loading takes longer than this, so fast
/// (cached) answers never flash it.
const SKELETON_DELAY: Duration = Duration::from_millis(150);

/// What a page shows: nothing (briefly), the skeleton, the content, an
/// empty state or an error.
pub struct PageStack {
    stack: gtk::Stack,
    timer: RefCell<Option<glib::SourceId>>,
}

impl PageStack {
    pub fn new(skeleton: &impl IsA<gtk::Widget>) -> Rc<Self> {
        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(200)
            .vexpand(true)
            .hexpand(true)
            .build();
        stack.add_named(&gtk::Box::new(gtk::Orientation::Vertical, 0), Some("blank"));
        stack.add_named(skeleton, Some("skeleton"));
        let this = Rc::new(Self {
            stack,
            timer: RefCell::default(),
        });
        this.show_loading();
        this
    }

    pub fn widget(&self) -> &gtk::Stack {
        &self.stack
    }

    fn cancel_timer(&self) {
        if let Some(timer) = self.timer.take() {
            timer.remove();
        }
    }

    fn set_child(&self, name: &str, child: &impl IsA<gtk::Widget>) {
        if let Some(old) = self.stack.child_by_name(name) {
            if &old == child.upcast_ref() {
                return;
            }
            self.stack.remove(&old);
        }
        self.stack.add_named(child, Some(name));
    }

    /// Shows the skeleton after a short delay (unless something else is
    /// shown first).
    pub fn show_loading(self: &Rc<Self>) {
        self.cancel_timer();
        self.stack.set_visible_child_name("blank");
        let weak = Rc::downgrade(self);
        let timer = glib::timeout_add_local_once(SKELETON_DELAY, move || {
            if let Some(this) = weak.upgrade() {
                this.timer.take();
                this.stack.set_visible_child_name("skeleton");
            }
        });
        self.timer.replace(Some(timer));
    }

    /// Sets (or replaces) the content widget without showing it.
    pub fn set_content(&self, content: &impl IsA<gtk::Widget>) {
        self.set_child("content", content);
    }

    pub fn show_content(&self) {
        self.cancel_timer();
        if self.stack.child_by_name("content").is_some() {
            self.stack.set_visible_child_name("content");
        }
    }

    pub fn is_showing_content(&self) -> bool {
        self.stack.visible_child_name().as_deref() == Some("content")
    }

    pub fn show_empty(&self, icon: &str, title: &str, description: &str) {
        self.cancel_timer();
        self.set_child("empty", &empty_status(icon, title, description));
        self.stack.set_visible_child_name("empty");
    }

    pub fn show_error(&self, err: &anyhow::Error, retry: impl Fn() + 'static) {
        self.cancel_timer();
        self.set_child("error", &error_status(&describe_error(err), retry));
        self.stack.set_visible_child_name("error");
    }
}

impl Drop for PageStack {
    fn drop(&mut self) {
        self.cancel_timer();
    }
}

/// An empty state.
pub fn empty_status(icon: &str, title: &str, description: &str) -> adw::StatusPage {
    adw::StatusPage::builder()
        .icon_name(icon)
        .title(title)
        .description(glib::markup_escape_text(description).as_str())
        .build()
}

/// "Couldn't Load This Page" with a Retry button (also F5).
pub fn error_status(description: &str, retry: impl Fn() + 'static) -> adw::StatusPage {
    let retry = Rc::new(retry);
    let button = gtk::Button::builder()
        .label("Retry")
        .halign(gtk::Align::Center)
        .css_classes(["pill", "suggested-action"])
        .build();
    button.connect_clicked({
        let retry = retry.clone();
        move |_| retry()
    });
    let status = adw::StatusPage::builder()
        .icon_name("network-error-symbolic")
        .title("Couldn't Load This Page")
        .description(glib::markup_escape_text(description).as_str())
        .child(&button)
        .build();
    let shortcuts = gtk::ShortcutController::new();
    shortcuts.set_scope(gtk::ShortcutScope::Managed);
    shortcuts.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("F5"),
        Some(gtk::CallbackAction::new(move |_, _| {
            retry();
            glib::Propagation::Stop
        })),
    ));
    status.add_controller(shortcuts);
    status
}

/// A sentence for an error, for status pages.
pub fn describe_error(err: &anyhow::Error) -> String {
    if crate::tidal::is_not_found(err) {
        return "TIDAL doesn't have this, or it isn't available in your country.".into();
    }
    if let Some(crate::tidal::Error::Api { message, .. }) = err.downcast_ref() {
        return format!("TIDAL said: {message}");
    }
    if err.chain().any(|cause| {
        cause
            .downcast_ref::<reqwest::Error>()
            .is_some_and(|e| e.is_connect() || e.is_timeout())
    }) {
        return "Check your internet connection and try again.".into();
    }
    format!("{err:#}")
}
