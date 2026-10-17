//! How counts and durations read everywhere (headers, subtitles, the
//! sidebar): "1,295 tracks · 3 d 7 h", "12 tracks · 42 min", "1 track".
//! Digits are grouped by thousands (with the UI language's separator);
//! parts are joined with " · ".

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

/// "1 track", "1,295 tracks" from untranslated words. Prefer [`counted`],
/// which translates.
pub fn count(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", group_digits(n), if n == 1 { one } else { many })
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
    match (days, hours, minutes) {
        (0, 0, m) => format!("{} min", m.max(1)),
        (0, h, 0) => format!("{h} h"),
        (0, h, m) => format!("{h} h {m} min"),
        (d, 0, _) => format!("{d} d"),
        (d, h, _) => format!("{d} d {h} h"),
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
        assert_eq!(count(1, "track", "tracks"), "1 track");
        assert_eq!(count(1295, "track", "tracks"), "1,295 tracks");
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
}
