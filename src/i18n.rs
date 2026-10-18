//! Translations of the user interface.
//!
//! The catalogues are standard gettext `.po` files in `po/`, embedded in the
//! binary, so Swell is translated without being installed. The language is
//! the one chosen in Preferences ([`init`]) or, by default, the system's:
//! `LANGUAGE`, `LC_ALL`, `LC_MESSAGES` or `LANG`, like gettext, and without
//! those (Windows, macOS apps opened from the Finder) the system's
//! preferred languages. English is the source language and the fallback
//! for any language without a catalogue. TIDAL's content is requested in the same language (see
//! [`tidal_locale`]), so a screen never mixes two languages.
//!
//! Use [`tr`] for plain strings, [`trn`] for plurals, and named
//! placeholders (`{name}`) replaced after translating, so translators can
//! reorder them:
//!
//! ```ignore
//! tr("Remove from This Playlist");
//! trn("{n} track", "{n} tracks", n).replace("{n}", &n.to_string());
//! ```

use std::collections::HashMap;
use std::sync::OnceLock;

/// Embedded catalogues: (language, .po source).
const CATALOGUES: &[(&str, &str)] = &[("es", include_str!("../po/es.po"))];

struct Catalogue {
    /// msgid → msgstr.
    singular: HashMap<&'static str, &'static str>,
    /// msgid → [singular form, plural form].
    plural: HashMap<&'static str, [&'static str; 2]>,
}

/// The choices of the Language preference (`settings.language`).
pub const PREFERENCES: [&str; 3] = ["auto", "en", "es"];

/// The preference this process started with (see [`init`]).
static PREFERENCE: OnceLock<&'static str> = OnceLock::new();
/// `LANGUAGE` as the user had it, before [`init`] changed it.
static ORIGINAL_LANGUAGE: OnceLock<Option<std::ffi::OsString>> = OnceLock::new();

/// Sets the language for this run from the saved preference: `"auto"`
/// (the system's), `"en"` or `"es"`; anything else counts as `"auto"`.
/// Call once at startup, before the first [`tr`]. Returns the gettext
/// language GTK's and libadwaita's own strings should use (to put in
/// `LANGUAGE`), when the preference forces one.
pub fn init(preference: &str) -> Option<&'static str> {
    let preference = PREFERENCES
        .into_iter()
        .find(|p| *p == preference)
        .unwrap_or("auto");
    let _ = PREFERENCE.set(preference);
    let _ = ORIGINAL_LANGUAGE.set(std::env::var_os("LANGUAGE"));
    (preference != "auto").then_some(preference)
}

/// The preference this process started with.
pub fn preference() -> &'static str {
    PREFERENCE.get().copied().unwrap_or("auto")
}

/// Restores the user's own `LANGUAGE` for a process started from this one
/// (a restart must detect the system language afresh).
pub fn restore_environment(command: &mut std::process::Command) {
    match ORIGINAL_LANGUAGE.get() {
        Some(Some(value)) => command.env("LANGUAGE", value),
        Some(None) => command.env_remove("LANGUAGE"),
        None => command,
    };
}

/// The catalogue language for a code or locale (`es`, `es_CL.UTF-8`,
/// `es-419`), if there is one.
fn supported(tag: &str) -> Option<&'static str> {
    let code = tag.split(['_', '-', '.', '@']).next()?;
    CATALOGUES
        .iter()
        .map(|(language, _)| *language)
        .find(|language| language.eq_ignore_ascii_case(code))
}

fn language() -> Option<&'static str> {
    // Tests check the English strings whatever the machine's locale is.
    if cfg!(test) {
        return None;
    }
    static LANGUAGE: OnceLock<Option<&'static str>> = OnceLock::new();
    *LANGUAGE.get_or_init(|| match preference() {
        "auto" => system_language(),
        forced => supported(forced),
    })
}

/// A non-empty environment variable.
fn var(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

/// The locale from the environment (`LC_ALL`, then `LC_MESSAGES`, then
/// `LANG`), like gettext.
fn env_locale() -> Option<String> {
    var("LC_ALL")
        .or_else(|| var("LC_MESSAGES"))
        .or_else(|| var("LANG"))
}

fn system_language() -> Option<&'static str> {
    // The user's LANGUAGE, not one set by `init` for GTK.
    let language_var = match ORIGINAL_LANGUAGE.get() {
        Some(original) => original
            .as_ref()
            .and_then(|v| v.to_str())
            .filter(|v| !v.is_empty())
            .map(str::to_owned),
        None => var("LANGUAGE"),
    };
    match env_locale() {
        // Like gettext: the locale decides first; "C"/"POSIX" means
        // untranslated even if LANGUAGE is set, and only then does
        // LANGUAGE pick among languages ("es_CL:es:en": the first we have).
        Some(locale) => {
            if locale == "C" || locale == "POSIX" || locale.starts_with("C.") {
                return None;
            }
            let wanted = language_var.unwrap_or(locale);
            wanted.split(':').find_map(supported)
        }
        // No locale variables (usual on Windows and for macOS apps): the
        // system's preferred languages, in order.
        None => language_var
            .iter()
            .flat_map(|list| list.split(':').map(str::to_owned).collect::<Vec<_>>())
            .chain(sys_locale::get_locales())
            .find_map(|tag| supported(&tag)),
    }
}

fn catalogue() -> Option<&'static Catalogue> {
    static CATALOGUE: OnceLock<Option<Catalogue>> = OnceLock::new();
    CATALOGUE
        .get_or_init(|| {
            let language = language()?;
            let source = CATALOGUES.iter().find(|(l, _)| *l == language)?.1;
            Some(parse(source))
        })
        .as_ref()
}

/// The UI language's code (`"es"`), or `None` for the built-in English.
/// Tests use it to expect the English strings.
#[cfg(test)]
pub fn ui_language() -> Option<&'static str> {
    language()
}

/// TIDAL's `locale` parameter matching the UI language: the user's own
/// regional variant when it is that language (`es_CL`), English otherwise.
pub fn tidal_locale() -> String {
    let Some(language) = language() else {
        return "en_US".to_owned();
    };
    // The environment's locale, then the system's preferred ones (BCP 47
    // on Windows and macOS: "es-CL").
    let regional = env_locale()
        .filter(|value| value != "C" && value != "POSIX")
        .into_iter()
        .chain(sys_locale::get_locales())
        .map(|value| {
            value
                .split(['.', '@'])
                .next()
                .unwrap_or("")
                .replace('-', "_")
        })
        .find(|tag| tag.len() == 5 && tag.starts_with(language) && tag.as_bytes()[2] == b'_');
    regional.unwrap_or_else(|| match language {
        "es" => "es_ES".to_owned(),
        other => format!("{other}_{}", other.to_ascii_uppercase()),
    })
}

/// The translation of `text`, or `text` itself.
pub fn tr(text: &'static str) -> &'static str {
    catalogue()
        .and_then(|catalogue| catalogue.singular.get(text).copied())
        .filter(|translated| !translated.is_empty())
        .unwrap_or(text)
}

/// `singular` or `plural` for `n`, translated.
pub fn trn(singular: &'static str, plural: &'static str, n: u64) -> &'static str {
    let one = n == 1;
    if let Some(forms) = catalogue().and_then(|catalogue| catalogue.plural.get(singular)) {
        // Spanish (the only catalogue so far) has gettext's default
        // "n != 1" rule, like English.
        let form = if one { forms[0] } else { forms[1] };
        if !form.is_empty() {
            return form;
        }
    }
    if one { singular } else { plural }
}

/// Parses the subset of the `.po` format that msgmerge writes: `msgid`,
/// `msgid_plural`, `msgstr`, `msgstr[0]`/`[1]`, continuation lines and
/// comments. The strings are leaked once: the catalogue lives as long as
/// the process.
fn parse(source: &str) -> Catalogue {
    #[derive(Default)]
    struct Entry {
        id: String,
        id_plural: Option<String>,
        strs: Vec<String>,
        fuzzy: bool,
    }
    enum Field {
        None,
        Id,
        IdPlural,
        Str(usize),
    }

    let mut catalogue = Catalogue {
        singular: HashMap::new(),
        plural: HashMap::new(),
    };
    let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
    let mut finish = |entry: Entry| {
        if entry.id.is_empty() || entry.fuzzy {
            return;
        }
        let id = leak(entry.id);
        match entry.id_plural {
            Some(_) => {
                let mut forms = entry.strs.into_iter().map(leak);
                let one = forms.next().unwrap_or("");
                let other = forms.next().unwrap_or("");
                catalogue.plural.insert(id, [one, other]);
            }
            None => {
                if let Some(text) = entry.strs.into_iter().next() {
                    catalogue.singular.insert(id, leak(text));
                }
            }
        }
    };

    let mut entry = Entry::default();
    let mut field = Field::None;
    for line in source.lines().map(str::trim) {
        if line.is_empty() {
            finish(std::mem::take(&mut entry));
            field = Field::None;
            continue;
        }
        if let Some(flags) = line.strip_prefix("#,") {
            entry.fuzzy |= flags.contains("fuzzy");
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let (keyword, rest) = match line.find(' ') {
            Some(at) if !line.starts_with('"') => (&line[..at], line[at..].trim()),
            _ => ("", line),
        };
        match keyword {
            "msgid" => {
                if !entry.strs.is_empty() {
                    finish(std::mem::take(&mut entry));
                }
                field = Field::Id;
            }
            "msgid_plural" => field = Field::IdPlural,
            "msgstr" => field = Field::Str(0),
            keyword if keyword.starts_with("msgstr[") => {
                let index = keyword[7..keyword.len() - 1].parse().unwrap_or(0);
                field = Field::Str(index);
            }
            _ => {}
        }
        let Some(text) = unquote(rest) else { continue };
        match field {
            Field::None => {}
            Field::Id => entry.id.push_str(&text),
            Field::IdPlural => entry
                .id_plural
                .get_or_insert_with(String::new)
                .push_str(&text),
            Field::Str(index) => {
                while entry.strs.len() <= index {
                    entry.strs.push(String::new());
                }
                entry.strs[index].push_str(&text);
            }
        }
    }
    finish(entry);
    catalogue
}

/// The contents of a `"..."` C string with its escapes resolved.
fn unquote(text: &str) -> Option<String> {
    let inner = text.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PO: &str = r#"
# A comment
msgid ""
msgstr ""
"Content-Type: text/plain; charset=UTF-8\n"

msgid "Play"
msgstr "Reproducir"

#, fuzzy
msgid "Shuffle"
msgstr "Aleatorio"

msgid "Long "
"text"
msgstr "Texto "
"largo"

msgid "{n} track"
msgid_plural "{n} tracks"
msgstr[0] "{n} pista"
msgstr[1] "{n} pistas"

msgid "Say \"hi\""
msgstr "Di \"hola\""
"#;

    #[test]
    fn parses_po() {
        let catalogue = parse(PO);
        assert_eq!(catalogue.singular.get("Play"), Some(&"Reproducir"));
        assert_eq!(catalogue.singular.get("Shuffle"), None, "fuzzy is skipped");
        assert_eq!(catalogue.singular.get("Long text"), Some(&"Texto largo"));
        assert_eq!(
            catalogue.plural.get("{n} track"),
            Some(&["{n} pista", "{n} pistas"])
        );
        assert_eq!(catalogue.singular.get("Say \"hi\""), Some(&"Di \"hola\""));
        assert!(!catalogue.singular.contains_key(""));
    }

    /// The Rust string literal starting at `text` (just after its opening
    /// quote), unescaped, with `\` line continuations removed, and the text
    /// after its closing quote.
    fn literal(text: &str) -> Option<(String, &str)> {
        let mut out = String::new();
        let mut chars = text.char_indices().peekable();
        while let Some((at, c)) = chars.next() {
            match c {
                '"' => return Some((out, &text[at + 1..])),
                '\\' => match chars.next()?.1 {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    '\n' => {
                        while chars.peek().is_some_and(|(_, c)| c.is_whitespace()) {
                            chars.next();
                        }
                    }
                    other => out.push(other),
                },
                c => out.push(c),
            }
        }
        None
    }

    /// The literal at the start of `text` (after whitespace) and the rest.
    fn string_at(text: &str) -> Option<(String, &str)> {
        literal(text.trim_start().strip_prefix('"')?)
    }

    /// Two comma-separated literals at the start of `text`.
    fn plural_pair(text: &str) -> Option<(String, String)> {
        let (one, rest) = string_at(text)?;
        let (many, _) = string_at(rest.trim_start().strip_prefix(',')?)?;
        Some((one, many))
    }

    /// The text after the comma that ends the argument `text` starts with
    /// (skipping nested brackets and strings), or `None` if the call ends
    /// first.
    fn skip_argument(text: &str) -> Option<&str> {
        let mut depth = 0usize;
        let mut rest = text;
        while let Some(c) = rest.chars().next() {
            let after = &rest[c.len_utf8()..];
            match c {
                '"' => {
                    rest = literal(after)?.1;
                    continue;
                }
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth = depth.checked_sub(1)?,
                ',' if depth == 0 => return Some(after),
                _ => {}
            }
            rest = after;
        }
        None
    }

    /// The text after each `marker` that is not the tail of a longer
    /// identifier (`from_str(` is not `tr(`).
    fn calls<'a>(source: &'a str, marker: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        source.match_indices(marker).filter_map(move |(at, _)| {
            let before = source[..at].chars().next_back();
            let ident = before.is_some_and(|c| c.is_alphanumeric() || c == '_');
            (!ident).then(|| &source[at + marker.len()..])
        })
    }

    /// The label of a `SortOption` tuple, `("id", "Label", true|false)`,
    /// starting at `text` (just after the opening parenthesis).
    fn sort_option(text: &str) -> Option<String> {
        let (id, rest) = string_at(text)?;
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
            return None;
        }
        let (label, rest) = string_at(rest.trim_start().strip_prefix(',')?)?;
        let rest = rest.trim_start().strip_prefix(',')?.trim_start();
        let rest = rest
            .strip_prefix("true")
            .or_else(|| rest.strip_prefix("false"))?;
        rest.trim_start().starts_with(')').then_some(label)
    }

    /// Every message in the sources has a Spanish translation, found with
    /// the same rules as `po/update-pot`: the literals of `tr("…")`,
    /// `trn("…", "…")` and `n_("…")` (also when they start on the line
    /// after the parenthesis), the plural pairs passed to the
    /// `counted(n, "…", "…")` and `unit(n, "…", "…")` helpers, and the
    /// labels of `SortOption` tuples.
    #[test]
    fn every_string_is_translated() {
        let catalogue = parse(CATALOGUES[0].1);
        let mut missing = Vec::new();
        let mut stack = vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") || path.ends_with("i18n.rs") {
                    continue;
                }
                let source = std::fs::read_to_string(&path).unwrap();
                let mut check = |id: String, plural: bool| {
                    let found = if plural {
                        catalogue.plural.contains_key(id.as_str())
                    } else {
                        catalogue.singular.contains_key(id.as_str())
                    };
                    if !found {
                        missing.push(format!("{}: {id:?}", path.display()));
                    }
                };
                for marker in ["tr(", "n_("] {
                    for rest in calls(&source, marker) {
                        if let Some((id, _)) = string_at(rest) {
                            check(id, false);
                        }
                    }
                }
                for rest in calls(&source, "trn(") {
                    if let Some((id, _)) = plural_pair(rest) {
                        check(id, true);
                    }
                }
                for helper in ["counted(", "unit("] {
                    for rest in calls(&source, helper) {
                        if let Some((id, _)) = skip_argument(rest).and_then(plural_pair) {
                            check(id, true);
                        }
                    }
                }
                for (at, _) in source.match_indices('(') {
                    if let Some(label) = sort_option(&source[at + 1..]) {
                        check(label, false);
                    }
                }
            }
        }
        assert!(
            missing.is_empty(),
            "untranslated strings:\n{}",
            missing.join("\n")
        );
    }

    #[test]
    fn finds_messages_passed_through_helpers() {
        let call = skip_argument(r#"tracks.len(), "{n} track", "{n} tracks")"#);
        assert_eq!(
            call.and_then(plural_pair),
            Some(("{n} track".to_owned(), "{n} tracks".to_owned()))
        );
        assert_eq!(
            skip_argument("n: usize, one: &str)").and_then(plural_pair),
            None
        );
        assert_eq!(
            sort_option(r#""added", "Date Added", true)"#).as_deref(),
            Some("Date Added")
        );
        assert_eq!(sort_option(r#""Added", "Date Added", true)"#), None);
        assert_eq!(sort_option(r#""a", "b", c)"#), None);
        assert_eq!(calls("from_str(x) tr(y)", "tr(").count(), 1);
    }

    #[test]
    fn embedded_catalogues_parse() {
        for (_, source) in CATALOGUES {
            let catalogue = parse(source);
            assert!(!catalogue.singular.is_empty() || source.lines().count() < 30);
        }
    }
}
