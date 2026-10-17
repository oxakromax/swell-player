//! Translations of the user interface.
//!
//! The catalogues are standard gettext `.po` files in `po/`, embedded in the
//! binary, so Swell is translated without being installed. The language
//! comes from `LANGUAGE`, `LC_ALL`, `LC_MESSAGES` or `LANG`, like gettext.
//! TIDAL's content is requested in the same language (see
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

fn language() -> Option<&'static str> {
    static LANGUAGE: OnceLock<Option<&'static str>> = OnceLock::new();
    *LANGUAGE.get_or_init(|| {
        let wanted = ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"]
            .into_iter()
            .filter_map(|key| std::env::var(key).ok())
            .find(|value| !value.is_empty())?;
        // LANGUAGE may list several ("es_CL:es:en"); take the first we have.
        wanted.split(':').find_map(|entry| {
            let code = entry.split(['_', '.', '@']).next()?;
            CATALOGUES
                .iter()
                .map(|(language, _)| *language)
                .find(|language| *language == code)
        })
    })
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
pub fn ui_language() -> Option<&'static str> {
    language()
}

/// TIDAL's `locale` parameter matching the UI language: the user's own
/// regional variant when it is that language (`es_CL`), English otherwise.
pub fn tidal_locale() -> String {
    let Some(language) = language() else {
        return "en_US".to_owned();
    };
    let regional = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .filter_map(|key| std::env::var(key).ok())
        .find(|value| !value.is_empty() && value != "C" && value != "POSIX")
        .map(|value| value.split(['.', '@']).next().unwrap_or("").to_owned())
        .filter(|tag| tag.len() == 5 && tag.starts_with(language) && tag.as_bytes()[2] == b'_');
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
            Field::IdPlural => entry.id_plural.get_or_insert_with(String::new).push_str(&text),
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

    #[test]
    fn embedded_catalogues_parse() {
        for (_, source) in CATALOGUES {
            let catalogue = parse(source);
            assert!(!catalogue.singular.is_empty() || source.lines().count() < 30);
        }
    }
}
