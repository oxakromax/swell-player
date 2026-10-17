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
        super::skeleton::animate_while_mapped(skeleton);
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

    /// The error state for `err` ([`error_page`]); the error itself goes
    /// to the log.
    pub fn show_error(&self, err: &anyhow::Error, retry: impl Fn() + 'static) {
        self.cancel_timer();
        self.set_child("error", &error_page(err, retry));
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

/// What went wrong, as far as the user is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// TIDAL does not have it (404), or not in this country.
    NotFound,
    /// TIDAL could not be reached.
    Offline,
    /// Anything else: worth retrying.
    Other,
}

impl Failure {
    pub fn of(err: &anyhow::Error) -> Self {
        if crate::tidal::is_not_found(err) {
            Self::NotFound
        } else if crate::ui::context::is_connection_error(err) {
            Self::Offline
        } else {
            Self::Other
        }
    }
}

/// The error state of a page (docs/design.md §3 "Estados comunes"):
/// "Not Available" with "Go Back" when TIDAL does not have it; "You're
/// Offline" or "Couldn't Load This Page" with a plain sentence and "Retry"
/// (also F5) otherwise. The error chain goes to the log, never on screen.
pub fn error_page(err: &anyhow::Error, retry: impl Fn() + 'static) -> adw::StatusPage {
    log::warn!("page failed to load: {err:#}");
    match Failure::of(err) {
        Failure::NotFound => not_found_status(),
        failure => retry_status(failure, &describe_error(err), retry),
    }
}

/// "Not Available" with a "Go Back" button (to Home when there is nothing
/// to go back to).
pub fn not_found_status() -> adw::StatusPage {
    let button = gtk::Button::builder()
        .label("Go Back")
        .halign(gtk::Align::Center)
        .css_classes(["pill"])
        .build();
    button.connect_clicked(|button| {
        let popped = button
            .ancestor(adw::NavigationView::static_type())
            .and_downcast::<adw::NavigationView>()
            .is_some_and(|view| view.navigation_stack().n_items() > 1 && view.pop());
        if !popped {
            let _ = button.activate_action("win.section", Some(&"home".to_variant()));
        }
    });
    adw::StatusPage::builder()
        .icon_name("swell-unavailable-symbolic")
        .title("Not Available")
        .description(describe(Failure::NotFound))
        .child(&button)
        .build()
}

/// "Couldn't Load This Page" with a Retry button (also F5).
pub fn error_status(description: &str, retry: impl Fn() + 'static) -> adw::StatusPage {
    retry_status(Failure::Other, description, retry)
}

fn retry_status(
    failure: Failure,
    description: &str,
    retry: impl Fn() + 'static,
) -> adw::StatusPage {
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
    // Bundled icons: a theme cannot swap them for full-colour ones.
    let (icon, title) = match failure {
        Failure::Offline => ("swell-offline-symbolic", "You're Offline"),
        _ => ("swell-error-symbolic", "Couldn't Load This Page"),
    };
    let status = adw::StatusPage::builder()
        .icon_name(icon)
        .title(title)
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

fn describe(failure: Failure) -> &'static str {
    match failure {
        Failure::NotFound => "TIDAL doesn't have this, or it isn't available in your country.",
        Failure::Offline => "Check your internet connection and try again.",
        Failure::Other => "Something went wrong. Try again in a moment.",
    }
}

/// A plain sentence for an error (status pages, rows that failed to load
/// more): never the raw error chain, which belongs in the log.
pub fn describe_error(err: &anyhow::Error) -> String {
    describe(Failure::of(err)).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_read_as_sentences() {
        let raw = anyhow::anyhow!("invalid type: null, expected u64 at line 1 column 42");
        assert_eq!(Failure::of(&raw), Failure::Other);
        assert!(!describe_error(&raw).contains("line 1"));
    }
}
