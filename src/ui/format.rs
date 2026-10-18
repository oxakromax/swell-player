//! How counts and durations read everywhere (headers, subtitles, the
//! sidebar): "1,295 tracks · 3 d 7 h", "12 tracks · 42 min", "1 track".
//! Digits are grouped by thousands (with the UI language's separator);
//! parts are joined with " · ".

use gtk::glib;

use crate::i18n::{tr, trn};

/// "1,317" in English, "1.317" in Spanish.
pub fn group_digits(n: usize) -> String {
    // The msgid is the English thousands separator.
    group_digits_with(n, tr(","))
}

fn group_digits_with(n: usize, separator: &str) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push_str(separator);
        }
        out.push(c);
    }
    out
}

/// "1 track", "1,295 tracks", translated: `one` and `many` are msgids
/// with a `{n}` placeholder (`counted(n, "{n} track", "{n} tracks")`).
pub fn counted(n: usize, one: &'static str, many: &'static str) -> String {
    trn(one, many, n as u64).replace("{n}", &group_digits(n))
}

/// A total length: "42 min", "5 h 12 min", "3 d 7 h" (minutes are dropped
/// past a day, and less than a minute reads "1 min").
pub fn total_duration(seconds: u64) -> String {
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3600;
    let minutes = (seconds % 3600) / 60;
    let (d, h, m) = (days.to_string(), hours.to_string(), minutes.to_string());
    match (days, hours, minutes) {
        (0, 0, m) => tr("{n} min").replace("{n}", &m.max(1).to_string()),
        (0, _, 0) => tr("{n} h").replace("{n}", &h),
        (0, _, _) => tr("{h} h {m} min").replace("{h}", &h).replace("{m}", &m),
        (_, 0, _) => tr("{n} d").replace("{n}", &d),
        _ => tr("{d} d {h} h").replace("{d}", &d).replace("{h}", &h),
    }
}

/// "12 tracks · 42 min": a track count and their length, either part left
/// out when unknown (zero).
pub fn tracks_summary(tracks: usize, seconds: u64) -> String {
    let mut parts = Vec::with_capacity(2);
    if tracks > 0 {
        parts.push(counted(tracks, "{n} track", "{n} tracks"));
    }
    if seconds > 0 {
        parts.push(total_duration(seconds));
    }
    join(parts)
}

/// Joins the non-empty `parts` with " · ".
pub fn join<S: AsRef<str>>(parts: impl IntoIterator<Item = S>) -> String {
    parts
        .into_iter()
        .filter(|part| !part.as_ref().is_empty())
        .map(|part| part.as_ref().to_owned())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// `time` written with `format` (strftime, as `glib::DateTime::format`
/// takes it; translate the format with [`tr`]). Month and weekday names
/// (`%B`, `%b`, `%A`, `%a`) are the UI language's, not the C library's:
/// `LC_TIME` may name another language than the interface (an English
/// desktop with Chilean dates). Short names are the first three letters of
/// the full ones, as in English and Spanish.
pub fn date(time: &glib::DateTime, format: &str) -> String {
    let month = month_name(time.month());
    let weekday = weekday_name(time.day_of_week());
    let short = |name: &str| name.chars().take(3).collect::<String>();
    let mut expanded = String::with_capacity(format.len() + 16);
    let mut chars = format.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            expanded.push(c);
            continue;
        }
        match chars.next() {
            Some('B') => expanded.push_str(month),
            Some('b' | 'h') => expanded.push_str(&short(month)),
            Some('A') => expanded.push_str(weekday),
            Some('a') => expanded.push_str(&short(weekday)),
            Some(other) => {
                expanded.push('%');
                expanded.push(other);
            }
            None => expanded.push_str("%%"),
        }
    }
    time.format(&expanded).map(String::from).unwrap_or_default()
}

/// The name of month 1–12, translated.
fn month_name(month: i32) -> &'static str {
    match month {
        1 => tr("January"),
        2 => tr("February"),
        3 => tr("March"),
        4 => tr("April"),
        5 => tr("May"),
        6 => tr("June"),
        7 => tr("July"),
        8 => tr("August"),
        9 => tr("September"),
        10 => tr("October"),
        11 => tr("November"),
        _ => tr("December"),
    }
}

/// The name of weekday 1–7 (Monday first), translated.
fn weekday_name(day: i32) -> &'static str {
    match day {
        1 => tr("Monday"),
        2 => tr("Tuesday"),
        3 => tr("Wednesday"),
        4 => tr("Thursday"),
        5 => tr("Friday"),
        6 => tr("Saturday"),
        _ => tr("Sunday"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_durations() {
        assert_eq!(group_digits_with(0, ","), "0");
        assert_eq!(group_digits_with(999, ","), "999");
        assert_eq!(group_digits_with(1317, ","), "1,317");
        assert_eq!(group_digits_with(1_234_567, "."), "1.234.567");
        if crate::i18n::ui_language().is_some() {
            // The rest checks the English texts.
            return;
        }
        assert_eq!(counted(1, "{n} track", "{n} tracks"), "1 track");
        assert_eq!(counted(1295, "{n} track", "{n} tracks"), "1,295 tracks");
        assert_eq!(total_duration(20), "1 min");
        assert_eq!(total_duration(42 * 60 + 10), "42 min");
        assert_eq!(total_duration(3600), "1 h");
        assert_eq!(total_duration(3900), "1 h 5 min");
        assert_eq!(total_duration(3 * 86_400 + 7 * 3600 + 60), "3 d 7 h");
        assert_eq!(tracks_summary(12, 42 * 60), "12 tracks · 42 min");
        assert_eq!(tracks_summary(0, 42 * 60), "42 min");
        assert_eq!(tracks_summary(1, 0), "1 track");
        assert_eq!(join(["2026", "", "12 tracks"]), "2026 · 12 tracks");
    }

    #[test]
    fn dates_use_the_ui_language_names() {
        if crate::i18n::ui_language().is_some() {
            return;
        }
        let time = glib::DateTime::from_utc(2013, 5, 17, 9, 5, 0.0).unwrap();
        assert_eq!(date(&time, "%B %-d, %Y"), "May 17, 2013");
        assert_eq!(date(&time, "%b %-d"), "May 17");
        assert_eq!(date(&time, "%a %H:%M"), "Fri 09:05");
        assert_eq!(date(&time, "%A, 100%%"), "Friday, 100%");
        let time = glib::DateTime::from_utc(2026, 9, 6, 0, 0, 0.0).unwrap();
        assert_eq!(date(&time, "%-d %b (%A)"), "6 Sep (Sunday)");
    }
}
