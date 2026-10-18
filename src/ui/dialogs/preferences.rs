//! The Preferences dialog (docs/design.md §3.14), bound to the settings
//! store: changes apply (and save) as they are made.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;

use super::super::context::Ctx;
use crate::i18n::{self, tr};
use crate::runtime;
use crate::session::LoginFlow;
use crate::settings::{Normalization, QualityPreference, Settings};

/// Three pages, so the view switcher fits the header bar even in narrow
/// windows: Playback (quality, processing, output), General (behaviour,
/// storage) and Account (who is signed in, blocked artists, Sign Out).
pub fn present(ctx: &Rc<Ctx>) {
    let dialog = adw::PreferencesDialog::builder()
        .search_enabled(true)
        .content_height(600)
        .build();
    dialog.add(&playback_page(ctx));
    dialog.add(&general_page(ctx, &dialog));
    dialog.add(&account_page(ctx, &dialog));
    if let Some(page) = super::super::debug::preferences_page() {
        dialog.set_visible_page_name(&page);
    }
    dialog.present(Some(&ctx.window));
}

fn playback_page(ctx: &Rc<Ctx>) -> adw::PreferencesPage {
    let current = ctx.settings.get();

    // Only the qualities this sign-in can stream are offered.
    let hires = ctx.api.flow == LoginFlow::Pkce;
    let mut qualities = Vec::new();
    if hires {
        qualities.push((tr("Hi-Res Lossless"), QualityPreference::Best));
    }
    qualities.extend([
        (tr("Lossless"), QualityPreference::Lossless),
        (tr("High"), QualityPreference::High),
        (tr("Low"), QualityPreference::Low),
    ]);
    let selected = match current.quality {
        QualityPreference::Best => 0,
        quality => qualities
            .iter()
            .position(|(_, q)| *q == quality)
            .unwrap_or(0),
    };
    let quality = combo_row(
        tr("Streaming Quality"),
        &qualities
            .iter()
            .map(|(label, _)| *label)
            .collect::<Vec<_>>(),
        selected,
    );
    if !hires {
        quality.set_subtitle(tr("Hi-Res Lossless requires browser sign-in"));
    }
    connect_combo(ctx, &quality, move |s, index| {
        s.quality = qualities[index].1;
    });
    let quality_group = adw::PreferencesGroup::builder()
        .title(tr("Quality"))
        .build();
    quality_group.add(&quality);

    let normalizations = [
        Normalization::Off,
        Normalization::Track,
        Normalization::Album,
    ];
    let normalization = combo_row(
        tr("Volume Normalisation"),
        &[tr("Off"), tr("Track"), tr("Album")],
        normalizations
            .iter()
            .position(|n| *n == current.normalization)
            .unwrap_or(0),
    );
    normalization.set_subtitle(tr("Off keeps the output bit-perfect"));
    connect_combo(ctx, &normalization, move |s, index| {
        s.normalization = normalizations[index];
    });
    let gapless = switch_row(
        ctx,
        tr("Gapless Playback"),
        None,
        current.gapless,
        |s, on| s.gapless = on,
    );
    let processing = adw::PreferencesGroup::builder()
        .title(tr("Processing"))
        .build();
    processing.add(&normalization);
    processing.add(&gapless);

    let page = adw::PreferencesPage::builder()
        .name("playback")
        .title(tr("Playback"))
        .icon_name("media-playback-start-symbolic")
        .build();
    page.add(&quality_group);
    page.add(&processing);
    page.add(&output_group(ctx));
    page
}

/// "Output Device": the system default or one of the audio sinks
/// GStreamer finds (PipeWire, PulseAudio, ALSA), listed off the main
/// thread.
fn output_group(ctx: &Rc<Ctx>) -> adw::PreferencesGroup {
    let names = gtk::StringList::new(&[tr("System Default")]);
    let row = adw::ComboRow::builder()
        .title(tr("Output Device"))
        .model(&names)
        .build();
    // The saved device is offered even while it is unplugged.
    let saved = ctx.settings.get().output_device;
    if let Some(saved) = &saved {
        names.append(saved);
        row.set_selected(1);
    }
    row.set_sensitive(false);
    let settings = ctx.settings.clone();
    let row_ref = row.downgrade();
    glib::spawn_future_local(async move {
        let found =
            runtime::spawn(async move { Ok(tokio::task::spawn_blocking(audio_sinks).await?) })
                .await
                .unwrap_or_else(|err| {
                    log::warn!("could not list audio devices: {err:#}");
                    Vec::new()
                });
        let Some(row) = row_ref.upgrade() else {
            return;
        };
        for name in found {
            if saved.as_deref() != Some(name.as_str()) {
                names.append(&name);
            }
        }
        row.set_sensitive(true);
        row.connect_selected_notify(move |row| {
            let device = row
                .selected_item()
                .and_downcast::<gtk::StringObject>()
                .map(|item| item.string().to_string())
                .filter(|_| row.selected() > 0);
            settings.update(|s| s.output_device = device);
        });
    });
    let group = adw::PreferencesGroup::builder().title(tr("Output")).build();
    group.add(&row);
    group
}

/// Display names of the audio outputs GStreamer can see.
fn audio_sinks() -> Vec<String> {
    use gst::prelude::{DeviceExt, DeviceMonitorExt, DeviceMonitorExtManual};
    if let Err(err) = gst::init() {
        log::warn!("GStreamer is unavailable: {err}");
        return Vec::new();
    }
    let monitor = gst::DeviceMonitor::new();
    monitor.add_filter(Some("Audio/Sink"), None);
    if monitor.start().is_err() {
        return Vec::new();
    }
    let mut names: Vec<String> = Vec::new();
    for device in monitor.devices() {
        let name = device.display_name().to_string();
        if !names.contains(&name) {
            names.push(name);
        }
    }
    monitor.stop();
    names
}

fn general_page(ctx: &Rc<Ctx>, dialog: &adw::PreferencesDialog) -> adw::PreferencesPage {
    let current = ctx.settings.get();
    let group = adw::PreferencesGroup::builder()
        .title(tr("Behaviour"))
        .build();
    group.add(&switch_row(
        ctx,
        tr("Run in Background"),
        Some(tr("Keep playing when the window is closed")),
        current.background,
        |s, on| s.background = on,
    ));
    group.add(&switch_row(
        ctx,
        tr("Track Change Notifications"),
        Some(tr("When the window is not focused")),
        current.notifications,
        |s, on| s.notifications = on,
    ));
    group.add(&switch_row(
        ctx,
        tr("Restore Queue on Start"),
        None,
        current.restore_session,
        |s, on| s.restore_session = on,
    ));
    group.add(&language_row(ctx, dialog));
    let page = adw::PreferencesPage::builder()
        .name("general")
        .title(tr("General"))
        .icon_name("preferences-system-symbolic")
        .build();
    page.add(&group);
    page.add(&storage_group(ctx, dialog));
    page
}

/// "Language": Automatic, English and every translation (each language
/// named in itself: Español, 日本語…). It takes effect at the next start,
/// which the toast offers.
fn language_row(ctx: &Rc<Ctx>, dialog: &adw::PreferencesDialog) -> adw::ComboRow {
    let (choices, labels): (Vec<&'static str>, Vec<&'static str>) =
        std::iter::once(("auto", tr("Automatic")))
            .chain(i18n::languages())
            .unzip();
    let current = ctx.settings.get().language;
    let row = combo_row(
        tr("Language"),
        &labels,
        choices.iter().position(|c| *c == current).unwrap_or(0),
    );
    let toast: Rc<RefCell<Option<adw::Toast>>> = Rc::default();
    let settings = ctx.settings.clone();
    let window = ctx.window.downgrade();
    let dialog = dialog.downgrade();
    row.connect_selected_notify(move |row| {
        let choice = choices[(row.selected() as usize).min(choices.len() - 1)];
        settings.update(|s| s.language = choice.to_owned());
        if let Some(old) = toast.take() {
            old.dismiss();
        }
        // Back to the language this run started with: nothing to restart.
        if choice == i18n::preference() {
            return;
        }
        let Some(dialog) = dialog.upgrade() else {
            return;
        };
        let restart = adw::Toast::builder()
            .title(tr("Restart Swell to change the language"))
            .button_label(tr("Restart"))
            .timeout(0)
            .build();
        let window = window.clone();
        restart.connect_button_clicked(move |_| {
            let app = window
                .upgrade()
                .and_then(|window| window.application())
                .and_downcast::<adw::Application>();
            if let Some(app) = app {
                super::super::window::restart(&app);
            }
        });
        dialog.add_toast(restart.clone());
        toast.replace(Some(restart));
    });
    row
}

fn cache_dir() -> PathBuf {
    glib::user_cache_dir().join("swell-player")
}

fn storage_group(ctx: &Rc<Ctx>, dialog: &adw::PreferencesDialog) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title(tr("Storage"))
        .description(tr("Downloaded artwork and saved answers from TIDAL, so pages open instantly and work offline."))
        .build();
    let covers = cache_row(
        dialog,
        CacheTexts {
            title: tr("Cover Cache"),
            question: tr("Clear the Cover Cache?"),
            consequence: tr("Artwork is downloaded again as you browse."),
            cleared: tr("Cover Cache cleared"),
            failed: tr("Couldn’t clear the Cover Cache"),
        },
        || vec![cache_dir().join("covers")],
        {
            move || async move {
                let dir = cache_dir().join("covers");
                runtime::spawn(async move {
                    tokio::task::spawn_blocking(move || match std::fs::remove_dir_all(&dir) {
                        Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err.into()),
                        _ => Ok(()),
                    })
                    .await?
                })
                .await
            }
        },
    );
    let api = ctx.api.clone();
    let library = cache_row(
        dialog,
        CacheTexts {
            title: tr("Library Cache"),
            question: tr("Clear the Library Cache?"),
            consequence: tr("Pages are loaded again from TIDAL. Your collection is not affected."),
            cleared: tr("Library Cache cleared"),
            failed: tr("Couldn’t clear the Library Cache"),
        },
        || {
            let dir = cache_dir();
            [
                "library.sqlite3",
                "library.sqlite3-wal",
                "library.sqlite3-shm",
            ]
            .iter()
            .map(|name| dir.join(name))
            .collect()
        },
        move || {
            let api = api.clone();
            async move {
                runtime::spawn(async move {
                    match api.cache().await {
                        Some(cache) => cache.prune(Duration::ZERO).await,
                        None => Ok(()),
                    }
                })
                .await
            }
        },
    );
    group.add(&covers);
    group.add(&library);
    group
}

/// What a cache row says (translated).
struct CacheTexts {
    title: &'static str,
    question: &'static str,
    consequence: &'static str,
    cleared: &'static str,
    failed: &'static str,
}

/// A row showing the size of `paths` with a "Clear" button that asks
/// first, runs `clear` and shows the new size.
fn cache_row<F, Fut>(
    dialog: &adw::PreferencesDialog,
    texts: CacheTexts,
    paths: fn() -> Vec<PathBuf>,
    clear: F,
) -> adw::ActionRow
where
    F: Fn() -> Fut + 'static,
    Fut: std::future::Future<Output = anyhow::Result<()>> + 'static,
{
    let row = adw::ActionRow::builder()
        .title(texts.title)
        .subtitle(tr("Calculating…"))
        .build();
    let button = gtk::Button::builder()
        .label(tr("_Clear"))
        .use_underline(true)
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    row.add_suffix(&button);
    let refresh = Rc::new({
        let row = row.downgrade();
        move || {
            let row = row.clone();
            glib::spawn_future_local(async move {
                let size = runtime::spawn(async move {
                    Ok(tokio::task::spawn_blocking(move || {
                        paths().iter().map(|p| size_of(p)).sum::<u64>()
                    })
                    .await?)
                })
                .await
                .unwrap_or(0);
                if let Some(row) = row.upgrade() {
                    row.set_subtitle(&glib::format_size(size));
                }
            });
        }
    });
    refresh();
    let clear = Rc::new(clear);
    let CacheTexts {
        title,
        question,
        consequence,
        cleared,
        failed,
    } = texts;
    button.connect_clicked({
        let dialog = dialog.downgrade();
        move |button| {
            let Some(dialog) = dialog.upgrade() else {
                return;
            };
            let alert = adw::AlertDialog::builder()
                .heading(question)
                .body(consequence)
                .close_response("cancel")
                .default_response("cancel")
                .build();
            alert.add_responses(&[("cancel", tr("_Cancel")), ("clear", tr("_Clear"))]);
            alert.set_response_appearance("clear", adw::ResponseAppearance::Destructive);
            let clear = clear.clone();
            let refresh = refresh.clone();
            let weak = dialog.downgrade();
            let button = button.downgrade();
            alert.connect_response(Some("clear"), move |_, _| {
                let clear = clear.clone();
                let refresh = refresh.clone();
                let weak = weak.clone();
                let button = button.clone();
                glib::spawn_future_local(async move {
                    if let Some(button) = button.upgrade() {
                        button.set_sensitive(false);
                    }
                    let result = clear().await;
                    if let Some(button) = button.upgrade() {
                        button.set_sensitive(true);
                    }
                    refresh();
                    let message = match result {
                        Ok(()) => cleared,
                        Err(err) => {
                            log::warn!("could not clear a cache ({title}): {err:#}");
                            failed
                        }
                    };
                    if let Some(dialog) = weak.upgrade() {
                        dialog.add_toast(adw::Toast::new(message));
                    }
                });
            });
            alert.present(Some(&dialog));
        }
    });
    row
}

/// Bytes used by a file or a whole directory tree.
fn size_of(path: &Path) -> u64 {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if !metadata.is_dir() {
        return metadata.len();
    }
    std::fs::read_dir(path)
        .map(|entries| entries.flatten().map(|entry| size_of(&entry.path())).sum())
        .unwrap_or(0)
}

/// The artists blocked with "Block Artist", each with "Unblock". (TIDAL's
/// API offers no list of blocked tracks, so there is none here.)
fn blocked_group(ctx: &Rc<Ctx>, dialog: &adw::PreferencesDialog) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title(tr("Blocked Artists"))
        .description(tr("Loading…"))
        .build();

    let api = ctx.api.clone();
    let group_ref = group.downgrade();
    let dialog = dialog.downgrade();
    let ctx = Rc::downgrade(ctx);
    let task = glib::spawn_future_local(async move {
        let result = runtime::spawn(async move { api.blocked_artists().await }).await;
        let Some(group) = group_ref.upgrade() else {
            return;
        };
        let artists = match result {
            Ok(page) => page.items,
            Err(err) => {
                log::warn!("could not load blocked artists: {err:#}");
                group.set_description(Some(tr("Couldn’t load blocked artists.")));
                return;
            }
        };
        group.set_description(Some(if artists.is_empty() {
            tr("Nothing blocked. Blocked artists are skipped in radios and mixes.")
        } else {
            tr("Skipped in radios and mixes.")
        }));
        for artist in artists {
            // Row titles are Pango markup.
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&artist.name).as_str())
                .build();
            row.add_prefix(&adw::Avatar::new(32, Some(&artist.name), true));
            let unblock = gtk::Button::builder()
                .label(tr("Unblock"))
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .build();
            row.add_suffix(&unblock);
            group.add(&row);
            let id = artist.id;
            let name = artist.name.clone();
            let ctx = ctx.clone();
            let dialog = dialog.clone();
            let group = group.downgrade();
            let row = row.downgrade();
            unblock.connect_clicked(move |button| {
                let Some(api) = ctx.upgrade().map(|ctx| ctx.api.clone()) else {
                    return;
                };
                button.set_sensitive(false);
                let button = button.downgrade();
                let row = row.clone();
                let ctx = ctx.clone();
                let dialog = dialog.clone();
                let group = group.clone();
                let name = name.clone();
                glib::spawn_future_local(async move {
                    match runtime::spawn(async move { api.set_artist_blocked(id, false).await })
                        .await
                    {
                        Ok(()) => {
                            if let (Some(group), Some(row)) = (group.upgrade(), row.upgrade()) {
                                group.remove(&row);
                            }
                            if let Some(dialog) = dialog.upgrade() {
                                dialog.add_toast(adw::Toast::new(
                                    &tr("Unblocked {name}")
                                        .replace("{name}", &glib::markup_escape_text(&name)),
                                ));
                            }
                        }
                        Err(err) => {
                            if let Some(button) = button.upgrade() {
                                button.set_sensitive(true);
                            }
                            if let Some(ctx) = ctx.upgrade() {
                                ctx.report(tr("Couldn’t unblock the artist"), &err);
                            }
                        }
                    }
                });
            });
        }
    });
    group.connect_destroy(move |_| task.abort());
    group
}

fn account_page(ctx: &Rc<Ctx>, dialog: &adw::PreferencesDialog) -> adw::PreferencesPage {
    let (method, quality) = match ctx.api.flow {
        LoginFlow::Pkce => (
            tr("Signed in with the browser"),
            tr("Up to Hi-Res Lossless"),
        ),
        LoginFlow::DeviceCode => (
            tr("Signed in with a device code"),
            tr("Up to Lossless (16-bit/44.1 kHz)"),
        ),
    };
    let avatar = adw::Avatar::builder()
        .size(64)
        .icon_name("avatar-default-symbolic")
        .build();
    let title = gtk::Label::builder()
        .label(method)
        .css_classes(["title-4"])
        .wrap(true)
        .build();
    let subtitle = gtk::Label::builder()
        .label(quality)
        .css_classes(["dimmed"])
        .wrap(true)
        .build();
    let header = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();
    header.append(&avatar);
    header.append(&title);
    header.append(&subtitle);
    let header_group = adw::PreferencesGroup::new();
    header_group.add(&header);

    let sign_out = adw::ButtonRow::builder()
        .title(tr("Sign _Out…"))
        .use_underline(true)
        .css_classes(["destructive-action"])
        .build();
    sign_out.connect_activated({
        let dialog = dialog.downgrade();
        let window = ctx.window.downgrade();
        move |_| {
            if let Some(dialog) = dialog.upgrade() {
                dialog.close();
            }
            if let Some(window) = window.upgrade() {
                let _ = WidgetExt::activate_action(&window, "win.sign-out", None);
            }
        }
    });
    let group = adw::PreferencesGroup::new();
    group.add(&sign_out);

    let page = adw::PreferencesPage::builder()
        .name("account")
        .title(tr("Account"))
        .icon_name("avatar-default-symbolic")
        .build();
    page.add(&header_group);
    page.add(&blocked_group(ctx, dialog));
    page.add(&group);
    page
}

fn combo_row(title: &str, labels: &[&str], selected: usize) -> adw::ComboRow {
    adw::ComboRow::builder()
        .title(title)
        .model(&gtk::StringList::new(labels))
        .selected(selected as u32)
        .build()
}

fn connect_combo(
    ctx: &Rc<Ctx>,
    row: &adw::ComboRow,
    apply: impl Fn(&mut Settings, usize) + 'static,
) {
    let settings = ctx.settings.clone();
    row.connect_selected_notify(move |row| {
        let index = row.selected() as usize;
        settings.update(|s| apply(s, index));
    });
}

fn switch_row(
    ctx: &Rc<Ctx>,
    title: &str,
    subtitle: Option<&str>,
    active: bool,
    apply: impl Fn(&mut Settings, bool) + 'static,
) -> adw::SwitchRow {
    let row = adw::SwitchRow::builder()
        .title(title)
        .active(active)
        .build();
    if let Some(subtitle) = subtitle {
        row.set_subtitle(subtitle);
    }
    let settings = ctx.settings.clone();
    row.connect_active_notify(move |row| {
        let active = row.is_active();
        settings.update(|s| apply(s, active));
    });
    row
}
