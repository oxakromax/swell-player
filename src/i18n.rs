//! Translations of the user interface.
//!
//! The catalogues are standard gettext `.po` files in `po/`, embedded in the
//! binary, so Swell is translated without being installed. The language is
//! the one chosen in Preferences ([`init`]) or, by default, the system's:
//! `LANGUAGE`, `LC_ALL`, `LC_MESSAGES` or `LANG`, like gettext, and without
//! those (Windows, macOS apps opened from the Finder) the system's
//! preferred languages. English is the source language and the fallback
//! for any language without a catalogue, and for any string a catalogue
//! does not translate. TIDAL's content is requested in the same language
//! (see [`tidal_locale`]), so a screen never mixes two languages.
//!
//! [`trn`] picks the plural form with each language's rule, written in
//! Rust in [`plural_form`] (the `Plural-Forms` headers are not evaluated;
//! a test checks they agree on the number of forms).
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

/// An embedded catalogue.
struct Language {
    /// The catalogue's language: an ISO 639 code, with the region when the
    /// catalogue is for one variant (`pt_BR`, `zh_TW`). It is also the
    /// Language preference's value and the gettext language GTK is given.
    code: &'static str,
    /// The language's name in itself, for the Language preference.
    name: &'static str,
    /// The `.po` source.
    source: &'static str,
}

/// Embedded catalogues, in the Language preference's order. Adding one also
/// needs its plural rule in [`plural_form`] and its number of forms in
/// `plural_count` (see docs/translating.md).
const CATALOGUES: &[Language] = &[
    Language {
        code: "es",
        name: "Español",
        source: include_str!("../po/es.po"),
    },
    Language {
        code: "fr",
        name: "Français",
        source: include_str!("../po/fr.po"),
    },
    Language {
        code: "de",
        name: "Deutsch",
        source: include_str!("../po/de.po"),
    },
    Language {
        code: "it",
        name: "Italiano",
        source: include_str!("../po/it.po"),
    },
    Language {
        code: "pt_BR",
        name: "Português (Brasil)",
        source: include_str!("../po/pt_BR.po"),
    },
    Language {
        code: "ru",
        name: "Русский",
        source: include_str!("../po/ru.po"),
    },
    Language {
        code: "ja",
        name: "日本語",
        source: include_str!("../po/ja.po"),
    },
    Language {
        code: "ko",
        name: "한국어",
        source: include_str!("../po/ko.po"),
    },
    Language {
        code: "zh_CN",
        name: "简体中文",
        source: include_str!("../po/zh_CN.po"),
    },
    Language {
        code: "zh_TW",
        name: "繁體中文",
        source: include_str!("../po/zh_TW.po"),
    },
];

/// The code of the source language, which has no catalogue.
const ENGLISH: &str = "en";

struct Catalogue {
    /// The catalogue's language code.
    language: &'static str,
    /// msgid → msgstr.
    singular: HashMap<&'static str, &'static str>,
    /// msgid → its forms, `msgstr[0]`, `msgstr[1]`…, as many as the
    /// language has (see [`plural_form`]).
    plural: HashMap<&'static str, Vec<&'static str>>,
}

/// The languages the Language preference offers besides "auto" (the
/// system's), in its order: `(code, name in that language)`, English first.
/// The code is what `settings.language` saves.
pub fn languages() -> impl Iterator<Item = (&'static str, &'static str)> {
    std::iter::once((ENGLISH, "English")).chain(CATALOGUES.iter().map(|l| (l.code, l.name)))
}

/// The preference this process started with (see [`init`]).
static PREFERENCE: OnceLock<&'static str> = OnceLock::new();
/// `LANGUAGE` as the user had it, before [`init`] changed it.
static ORIGINAL_LANGUAGE: OnceLock<Option<std::ffi::OsString>> = OnceLock::new();

/// Sets the language for this run from the saved preference: `"auto"`
/// (the system's) or one of [`languages`]' codes (`"en"`, `"es"`,
/// `"pt_BR"`…); anything else counts as `"auto"`. Call once at startup,
/// before the first [`tr`]. Returns the gettext language GTK's and
/// libadwaita's own strings should use (to put in `LANGUAGE`), when the
/// preference forces one.
pub fn init(preference: &str) -> Option<&'static str> {
    let preference = languages()
        .map(|(code, _)| code)
        .find(|code| *code == preference)
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

/// The language a code or locale asks for (`es`, `es_CL.UTF-8`, `es-419`,
/// `zh-Hant-TW`): a catalogue's code, [`ENGLISH`], or `None` for a
/// language Swell doesn't have. Chinese is Traditional (`zh_TW`) for
/// Taiwan, Hong Kong, Macau and the Hant script, Simplified (`zh_CN`)
/// otherwise; every Portuguese is `pt_BR`.
fn supported(tag: &str) -> Option<&'static str> {
    let tag = tag
        .split(['.', '@'])
        .next()?
        .replace('-', "_")
        .to_ascii_lowercase();
    let mut parts = tag.split('_');
    let code = match parts.next()? {
        "en" => return Some(ENGLISH),
        "zh" => {
            let parts: Vec<&str> = parts.collect();
            let traditional = !parts.contains(&"hans")
                && parts
                    .iter()
                    .any(|part| matches!(*part, "hant" | "tw" | "hk" | "mo"));
            if traditional { "zh_tw" } else { "zh_cn" }
        }
        "pt" => "pt_br",
        other => other,
    };
    CATALOGUES
        .iter()
        .map(|language| language.code)
        .find(|language| language.eq_ignore_ascii_case(code))
}

/// The UI language's catalogue code, or `None` for the built-in English.
fn language() -> Option<&'static str> {
    // Tests check the English strings whatever the machine's locale is.
    if cfg!(test) {
        return None;
    }
    static LANGUAGE: OnceLock<Option<&'static str>> = OnceLock::new();
    *LANGUAGE.get_or_init(|| {
        match preference() {
            "auto" => system_language(),
            forced => supported(forced),
        }
        .filter(|code| *code != ENGLISH)
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

/// The system's language: a catalogue's code, [`ENGLISH`] or `None`.
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
        // LANGUAGE pick among languages ("es_CL:es:en": the first we have,
        // English included).
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
            let entry = CATALOGUES.iter().find(|l| l.code == language)?;
            Some(parse(entry.code, entry.source))
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
/// regional variant when it is that language (`es_CL`, `fr_CA`), the
/// language's usual one otherwise, and `en_US` for English.
pub fn tidal_locale() -> String {
    let Some(language) = language() else {
        return "en_US".to_owned();
    };
    // The environment's locale, then the system's preferred ones (BCP 47
    // on Windows and macOS: "es-CL").
    let candidates = env_locale()
        .filter(|value| value != "C" && value != "POSIX")
        .into_iter()
        .chain(sys_locale::get_locales());
    tidal_locale_for(language, candidates)
}

/// [`tidal_locale`] for a catalogue language, given the user's locales.
fn tidal_locale_for(language: &str, candidates: impl Iterator<Item = String>) -> String {
    // A catalogue for one variant (pt_BR, zh_TW) asks for that variant,
    // whatever the user's region.
    let regional = (language.len() == 2)
        .then(|| {
            candidates
                .map(|value| {
                    value
                        .split(['.', '@'])
                        .next()
                        .unwrap_or("")
                        .replace('-', "_")
                })
                .find(|tag| {
                    tag.len() == 5 && tag.starts_with(language) && tag.as_bytes()[2] == b'_'
                })
        })
        .flatten();
    regional.unwrap_or_else(|| {
        match language {
            "es" => "es_ES",
            "fr" => "fr_FR",
            "de" => "de_DE",
            "it" => "it_IT",
            "pt_BR" => "pt_BR",
            "ru" => "ru_RU",
            "ja" => "ja_JP",
            "ko" => "ko_KR",
            "zh_CN" => "zh_CN",
            "zh_TW" => "zh_TW",
            other if other.len() == 2 => {
                return format!("{other}_{}", other.to_ascii_uppercase());
            }
            other => other,
        }
        .to_owned()
    })
}

/// How many plural forms a catalogue language has: the `nplurals` of its
/// `Plural-Forms` header. Tests check it against the headers and against
/// [`plural_form`].
#[cfg(test)]
fn plural_count(language: &str) -> usize {
    match language {
        "ru" => 3,
        "ja" | "ko" | "zh_CN" | "zh_TW" => 1,
        _ => 2,
    }
}

/// The plural form (`msgstr[i]`) a catalogue language uses for `n`: the
/// `plural=` expression of its `Plural-Forms` header, written in Rust (the
/// headers themselves are not evaluated).
fn plural_form(language: &str, n: u64) -> usize {
    match language {
        // plural=(n > 1): 0 and 1 are singular.
        "fr" | "pt_BR" => usize::from(n > 1),
        // plural=(n%10==1 && n%100!=11 ? 0 : n%10>=2 && n%10<=4 &&
        // (n%100<10 || n%100>=20) ? 1 : 2)
        "ru" => {
            let (units, tens) = (n % 10, n % 100);
            if units == 1 && tens != 11 {
                0
            } else if (2..=4).contains(&units) && !(12..=14).contains(&tens) {
                1
            } else {
                2
            }
        }
        // nplurals=1; plural=0;
        "ja" | "ko" | "zh_CN" | "zh_TW" => 0,
        // plural=(n != 1), like English: Spanish, German, Italian.
        _ => usize::from(n != 1),
    }
}

/// The translation of `text`, or `text` itself.
pub fn tr(text: &'static str) -> &'static str {
    catalogue()
        .and_then(|catalogue| catalogue.singular.get(text).copied())
        .filter(|translated| !translated.is_empty())
        .unwrap_or(text)
}

/// `singular` or `plural` for `n`, translated: the form the UI language
/// uses for `n`, or the English one when that form is not translated.
pub fn trn(singular: &'static str, plural: &'static str, n: u64) -> &'static str {
    let translated = catalogue().and_then(|catalogue| {
        let forms = catalogue.plural.get(singular)?;
        forms
            .get(plural_form(catalogue.language, n))
            .copied()
            .filter(|form| !form.is_empty())
    });
    translated.unwrap_or(if n == 1 { singular } else { plural })
}

/// Parses the subset of the `.po` format that msgmerge writes: `msgid`,
/// `msgid_plural`, `msgstr`, `msgstr[N]`, continuation lines and comments.
/// The strings are leaked once: the catalogue lives as long as the process.
fn parse(language: &'static str, source: &str) -> Catalogue {
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
        language,
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
                let forms = entry.strs.into_iter().map(leak).collect();
                catalogue.plural.insert(id, forms);
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
            keyword if keyword.starts_with("msgstr[") && keyword.ends_with(']') => {
                // At most a handful of forms: a bad index is not a reason
                // to allocate a huge vector.
                let index = keyword[7..keyword.len() - 1].parse().unwrap_or(0);
                field = Field::Str(index.min(15));
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
    use std::collections::BTreeSet;

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
        let catalogue = parse("es", PO);
        assert_eq!(catalogue.singular.get("Play"), Some(&"Reproducir"));
        assert_eq!(catalogue.singular.get("Shuffle"), None, "fuzzy is skipped");
        assert_eq!(catalogue.singular.get("Long text"), Some(&"Texto largo"));
        assert_eq!(
            catalogue.plural.get("{n} track"),
            Some(&vec!["{n} pista", "{n} pistas"])
        );
        assert_eq!(catalogue.singular.get("Say \"hi\""), Some(&"Di \"hola\""));
        assert!(!catalogue.singular.contains_key(""));
    }

    #[test]
    fn parses_any_number_of_plural_forms() {
        let catalogue = parse(
            "ru",
            r#"
msgid "{n} track"
msgid_plural "{n} tracks"
msgstr[0] "{n} трек"
msgstr[1] "{n} трека"
msgstr[2] "{n} треков"

msgid "{n} album"
msgid_plural "{n} albums"
msgstr[0] "{n} アルバム"
"#,
        );
        assert_eq!(
            catalogue.plural.get("{n} track"),
            Some(&vec!["{n} трек", "{n} трека", "{n} треков"])
        );
        assert_eq!(
            catalogue.plural.get("{n} album"),
            Some(&vec!["{n} アルバム"])
        );
    }

    /// The form chosen for 0, 1, 2…, as a list.
    fn forms(language: &str, numbers: &[u64]) -> Vec<usize> {
        numbers.iter().map(|&n| plural_form(language, n)).collect()
    }

    #[test]
    fn plural_rules() {
        let numbers = [0, 1, 2, 5, 11, 21, 101];
        // n != 1
        for language in ["en", "es", "de", "it"] {
            assert_eq!(
                forms(language, &numbers),
                [1, 0, 1, 1, 1, 1, 1],
                "{language}"
            );
            assert_eq!(plural_count(language), 2);
        }
        // n > 1: 0 and 1 are singular.
        for language in ["fr", "pt_BR"] {
            assert_eq!(
                forms(language, &numbers),
                [0, 0, 1, 1, 1, 1, 1],
                "{language}"
            );
            assert_eq!(plural_count(language), 2);
        }
        // One form.
        for language in ["ja", "ko", "zh_CN", "zh_TW"] {
            assert_eq!(forms(language, &numbers), [0; 7], "{language}");
            assert_eq!(plural_count(language), 1);
        }
    }

    #[test]
    fn russian_plural_rule() {
        assert_eq!(plural_count("ru"), 3);
        // One: 1, 21, 31… but not 11.
        for n in [1, 21, 31, 101, 121, 1001] {
            assert_eq!(plural_form("ru", n), 0, "{n}");
        }
        // Few: 2–4, 22–24… but not 12–14.
        for n in [2, 3, 4, 22, 23, 24, 102, 1004] {
            assert_eq!(plural_form("ru", n), 1, "{n}");
        }
        // Many: 0, 5–20, 25–30, 111…
        for n in [
            0, 5, 9, 10, 11, 12, 13, 14, 15, 19, 20, 25, 100, 111, 112, 114,
        ] {
            assert_eq!(plural_form("ru", n), 2, "{n}");
        }
    }

    #[test]
    fn every_catalogue_has_a_plural_rule() {
        for language in CATALOGUES {
            let count = plural_count(language.code);
            for n in 0..1000 {
                assert!(
                    plural_form(language.code, n) < count,
                    "{} {n}",
                    language.code
                );
            }
            // Every form is used.
            let used: BTreeSet<usize> = (0..1000).map(|n| plural_form(language.code, n)).collect();
            assert_eq!(used.len(), count, "{}", language.code);
        }
    }

    #[test]
    fn maps_locales_to_catalogues() {
        let cases = [
            ("es", Some("es")),
            ("es_CL.UTF-8", Some("es")),
            ("es-419", Some("es")),
            ("ES", Some("es")),
            ("en_US.UTF-8", Some("en")),
            ("en-GB", Some("en")),
            ("fr_CA.UTF-8", Some("fr")),
            ("de-AT", Some("de")),
            ("it_IT@euro", Some("it")),
            ("ru_RU.UTF-8", Some("ru")),
            ("ja-JP", Some("ja")),
            ("ko_KR.UTF-8", Some("ko")),
            ("pt", Some("pt_BR")),
            ("pt_BR.UTF-8", Some("pt_BR")),
            ("pt_PT", Some("pt_BR")),
            ("pt-BR", Some("pt_BR")),
            ("zh", Some("zh_CN")),
            ("zh_CN.UTF-8", Some("zh_CN")),
            ("zh-SG", Some("zh_CN")),
            ("zh-Hans", Some("zh_CN")),
            ("zh-Hans-CN", Some("zh_CN")),
            ("zh-Hans-HK", Some("zh_CN")),
            ("zh_TW.UTF-8", Some("zh_TW")),
            ("zh_HK.UTF-8", Some("zh_TW")),
            ("zh_MO", Some("zh_TW")),
            ("zh-Hant", Some("zh_TW")),
            ("zh-Hant-TW", Some("zh_TW")),
            ("zh-Hant-HK", Some("zh_TW")),
            ("zh_TW", Some("zh_TW")),
            ("nl_NL.UTF-8", None),
            ("C", None),
            ("", None),
        ];
        for (tag, expected) in cases {
            assert_eq!(supported(tag), expected, "{tag}");
        }
        // Every preference value maps to itself.
        for (code, _) in languages() {
            assert_eq!(supported(code), Some(code));
        }
    }

    #[test]
    fn tidal_locales() {
        let none = || std::iter::empty::<String>();
        let expected = [
            ("es", "es_ES"),
            ("fr", "fr_FR"),
            ("de", "de_DE"),
            ("it", "it_IT"),
            ("pt_BR", "pt_BR"),
            ("ru", "ru_RU"),
            ("ja", "ja_JP"),
            ("ko", "ko_KR"),
            ("zh_CN", "zh_CN"),
            ("zh_TW", "zh_TW"),
        ];
        for (language, locale) in expected {
            assert_eq!(tidal_locale_for(language, none()), locale);
        }
        let mine = |tags: &[&str]| tags.iter().map(|t| (*t).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            tidal_locale_for("es", mine(&["es_CL.UTF-8"]).into_iter()),
            "es_CL"
        );
        assert_eq!(
            tidal_locale_for("fr", mine(&["en-US", "fr-CA"]).into_iter()),
            "fr_CA"
        );
        // A catalogue for one variant keeps it.
        assert_eq!(
            tidal_locale_for("pt_BR", mine(&["pt_PT.UTF-8"]).into_iter()),
            "pt_BR"
        );
        assert_eq!(
            tidal_locale_for("zh_TW", mine(&["zh_HK.UTF-8"]).into_iter()),
            "zh_TW"
        );
        // Every catalogue has a locale of TIDAL's shape.
        for language in CATALOGUES {
            let locale = tidal_locale_for(language.code, none());
            assert!(
                locale.len() == 5 && locale.as_bytes()[2] == b'_',
                "{locale}"
            );
        }
    }

    #[test]
    fn languages_for_the_preference() {
        let codes: Vec<&str> = languages().map(|(code, _)| code).collect();
        assert_eq!(
            codes,
            [
                "en", "es", "fr", "de", "it", "pt_BR", "ru", "ja", "ko", "zh_CN", "zh_TW"
            ]
        );
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

    /// A message found in the sources.
    struct Message {
        /// The file it is in.
        file: String,
        id: String,
        /// The `msgid_plural`, for plurals.
        plural: Option<String>,
    }

    /// Every message in the sources, found with the same rules as
    /// `po/update-pot`: the literals of `tr("…")`, `trn("…", "…")` and
    /// `n_("…")` (also when they start on the line after the parenthesis),
    /// the plural pairs passed to the `counted(n, "…", "…")` and
    /// `unit(n, "…", "…")` helpers, and the labels of `SortOption` tuples.
    fn source_messages() -> Vec<Message> {
        let mut messages = Vec::new();
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
                let mut add = |id: String, plural: Option<String>| {
                    let file = path.display().to_string();
                    messages.push(Message { file, id, plural });
                };
                for marker in ["tr(", "n_("] {
                    for rest in calls(&source, marker) {
                        if let Some((id, _)) = string_at(rest) {
                            add(id, None);
                        }
                    }
                }
                for rest in calls(&source, "trn(") {
                    if let Some((id, plural)) = plural_pair(rest) {
                        add(id, Some(plural));
                    }
                }
                for helper in ["counted(", "unit("] {
                    for rest in calls(&source, helper) {
                        if let Some((id, plural)) = skip_argument(rest).and_then(plural_pair) {
                            add(id, Some(plural));
                        }
                    }
                }
                for (at, _) in source.match_indices('(') {
                    if let Some(label) = sort_option(&source[at + 1..]) {
                        add(label, None);
                    }
                }
            }
        }
        messages
    }

    /// A field of a catalogue's header (`Language`, `Plural-Forms`…).
    /// msgmerge wraps long fields over several quoted lines, so the
    /// header's lines are joined before it is split into fields.
    fn header_field(source: &str, name: &str) -> Option<String> {
        let header = source
            .split("\n\n")
            .find(|block| block.contains("msgid \"\"\n"))?;
        let text: String = header
            .lines()
            .filter_map(|line| line.trim().strip_prefix('"')?.strip_suffix('"'))
            .collect();
        text.split("\\n").find_map(|field| {
            let (key, value) = field.split_once(':')?;
            (key.trim() == name).then(|| value.trim().to_owned())
        })
    }

    /// A catalogue that does not translate every message yet: its header
    /// has `X-Swell-Partial: yes`. Used for the placeholder catalogues of
    /// languages whose translation has not landed; a real catalogue drops
    /// the field (see docs/translating.md).
    fn is_partial(source: &str) -> bool {
        header_field(source, "X-Swell-Partial").as_deref() == Some("yes")
    }

    /// Every message in the sources is translated in every catalogue that
    /// is not [partial](is_partial): a non-empty `msgstr`, or all the
    /// plural forms.
    #[test]
    fn every_string_is_translated() {
        let messages = source_messages();
        assert!(messages.len() > 100, "the source scan found too little");
        let mut missing = Vec::new();
        for language in CATALOGUES.iter().filter(|l| !is_partial(l.source)) {
            let catalogue = parse(language.code, language.source);
            for message in &messages {
                let translated = match message.plural {
                    Some(_) => catalogue
                        .plural
                        .get(message.id.as_str())
                        .is_some_and(|forms| forms.iter().all(|form| !form.is_empty())),
                    None => catalogue
                        .singular
                        .get(message.id.as_str())
                        .is_some_and(|text| !text.is_empty()),
                };
                if !translated {
                    missing.push(format!(
                        "{} ({}): {:?}",
                        message.file, language.code, message.id
                    ));
                }
            }
        }
        assert!(
            missing.is_empty(),
            "untranslated strings:\n{}",
            missing.join("\n")
        );
    }

    /// The `{name}` placeholders in `text`.
    fn placeholders(text: &str) -> BTreeSet<&str> {
        let mut found = BTreeSet::new();
        let mut rest = text;
        while let Some(open) = rest.find('{') {
            rest = &rest[open + 1..];
            if let Some(close) = rest.find('}') {
                let name = &rest[..close];
                if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    found.insert(name);
                    rest = &rest[close + 1..];
                }
            }
        }
        found
    }

    /// Every catalogue (partial ones included) declares its language and
    /// the plural forms [`plural_form`] uses, every plural entry has that
    /// many forms, and every translation keeps its message's placeholders:
    /// the same ones, except that a plural form used only for n = 1 may
    /// leave out some (`"Skipped a track"` for `"Skipped {n} tracks"`).
    #[test]
    fn catalogues_are_consistent() {
        let plurals: HashMap<String, String> = source_messages()
            .into_iter()
            .filter_map(|m| Some((m.id, m.plural?)))
            .collect();
        let mut problems = Vec::new();
        for language in CATALOGUES {
            let code = language.code;
            let mut problem = |text: String| problems.push(format!("{code}: {text}"));
            if header_field(language.source, "Language").as_deref() != Some(code) {
                problem("the header's Language is not the catalogue's code".to_owned());
            }
            let count = plural_count(code);
            let nplurals = header_field(language.source, "Plural-Forms").and_then(|forms| {
                forms.split(';').find_map(|f| {
                    f.trim()
                        .strip_prefix("nplurals=")?
                        .trim()
                        .parse::<usize>()
                        .ok()
                })
            });
            if nplurals != Some(count) {
                problem(format!(
                    "Plural-Forms has nplurals={nplurals:?}, plural_form() uses {count}"
                ));
            }
            let catalogue = parse(code, language.source);
            for (id, text) in &catalogue.singular {
                if !text.is_empty() && placeholders(text) != placeholders(id) {
                    problem(format!("placeholders differ: {id:?} → {text:?}"));
                }
            }
            for (id, forms) in &catalogue.plural {
                if forms.len() != count {
                    problem(format!("{id:?} has {} forms, not {count}", forms.len()));
                }
                // An obsolete message: nothing to compare with.
                let Some(plural) = plurals.get(*id) else {
                    continue;
                };
                let all: BTreeSet<&str> = placeholders(id)
                    .union(&placeholders(plural))
                    .copied()
                    .collect();
                for (index, form) in forms.iter().enumerate().filter(|(_, f)| !f.is_empty()) {
                    let only_one = (0..1000)
                        .filter(|&n| plural_form(code, n) == index)
                        .all(|n| n == 1);
                    let found = placeholders(form);
                    let right = if only_one {
                        found.is_subset(&all)
                    } else {
                        found == placeholders(plural)
                    };
                    if !right {
                        problem(format!(
                            "placeholders differ: {plural:?} → [{index}] {form:?}"
                        ));
                    }
                }
            }
        }
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }

    #[test]
    fn finds_placeholders() {
        let names = |text| placeholders(text).into_iter().collect::<Vec<_>>();
        assert_eq!(
            names("{bits}-bit/{rate} kHz {codec}"),
            ["bits", "codec", "rate"]
        );
        assert_eq!(names("{n} track"), ["n"]);
        assert_eq!(names("no {braces here} {}"), Vec::<&str>::new());
        assert_eq!(names("{a}{a}"), ["a"]);
    }

    #[test]
    fn reads_catalogue_headers() {
        let source = "# x\nmsgid \"\"\nmsgstr \"\"\n\"Language: pt_BR\\n\"\n\
                      \"X-Swell-Partial: yes\\n\"\n\nmsgid \"a\"\nmsgstr \"\"\n\"Language: no\\n\"\n";
        assert_eq!(header_field(source, "Language").as_deref(), Some("pt_BR"));
        // msgmerge wraps long fields.
        let wrapped = "msgid \"\"\nmsgstr \"\"\n\"Plural-Forms: nplurals=3; plural=(n%10==1 && \"\n\
                       \"n%100!=11 ? 0 : 2);\\n\"\n\"Language: ru\\n\"\n";
        assert_eq!(
            header_field(wrapped, "Plural-Forms").as_deref(),
            Some("nplurals=3; plural=(n%10==1 && n%100!=11 ? 0 : 2);")
        );
        assert_eq!(header_field(wrapped, "Language").as_deref(), Some("ru"));
        assert!(is_partial(source));
        assert!(!is_partial(PO));
        // The Spanish catalogue is complete.
        assert!(!is_partial(CATALOGUES[0].source));
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
        for language in CATALOGUES {
            let catalogue = parse(language.code, language.source);
            assert!(
                !catalogue.singular.is_empty() || is_partial(language.source),
                "{}",
                language.code
            );
        }
    }
}
