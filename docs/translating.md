# Translating Swell

Swell's interface is written in English and translated with standard
gettext `.po` files in [`po/`](../po). The catalogues are embedded in the
binary when it is built, so no `.mo` files or installation are needed.
Swell picks the language from the system (`LANGUAGE`, `LC_ALL`,
`LC_MESSAGES`, `LANG`, like gettext) unless the user chooses one in
Preferences, and asks TIDAL for its content (editorial texts, page titles)
in the same language.

| File | What it is |
|---|---|
| `po/swell.pot` | The template: every translatable string, untranslated. Generated, don't edit it. |
| `po/update-pot` | Regenerates the template from `src/` (Python 3, no dependencies). |
| `po/es.po` | Spanish. |

Any `.po` editor works: [Poedit](https://poedit.net),
[GNOME Translation Editor](https://wiki.gnome.org/Apps/Gtranslator),
[Lokalize](https://apps.kde.org/lokalize/), or a plain text editor.

## Improving an existing translation

Edit `po/<language>.po`, rebuild, and check the result (see
[Trying it out](#trying-it-out)). Entries marked `#, fuzzy` are ignored by
Swell: review them and remove the flag once they are right.

## Adding a language

The steps below use French (`fr`) as the example; use your language's
ISO 639 code.

1. **Create the catalogue** from the template:

   ```bash
   cp po/swell.pot po/fr.po
   ```

   (or `msginit -i po/swell.pot -l fr -o po/fr.po --no-translator`). In the
   header at the top, set `Language: fr`, fill in `Last-Translator` and
   `Language-Team` if you like, set `Plural-Forms` for your language, and
   remove the `#, fuzzy` line above the header.

2. **Translate** every `msgstr`. See [Guidelines](#guidelines) below.

3. **Register the catalogue** in `CATALOGUES` in
   [`src/i18n.rs`](../src/i18n.rs), which embeds it in the binary:

   ```rust
   const CATALOGUES: &[(&str, &str)] = &[
       ("es", include_str!("../po/es.po")),
       ("fr", include_str!("../po/fr.po")),
   ];
   ```

4. **Add it to the Language preference** in
   [`src/ui/dialogs/preferences.rs`](../src/ui/dialogs/preferences.rs),
   with the language's own name ("Français", not "French"), so people can
   choose it regardless of their system language.

5. **Check TIDAL's locale.** `tidal_locale()` in `src/i18n.rs` asks TIDAL
   for content in the user's regional variant (`fr_CA`) or, failing that,
   in `<code>_<CODE>` (`fr_FR`). If that default is wrong for your
   language (Portuguese probably wants `pt_BR`), add a case for it next to
   Spanish's.

6. **Plural forms.** `trn()` currently chooses between two forms with
   English's rule (`n != 1`), which is also right for Spanish, German,
   Italian and many others. If your language uses another rule (French
   treats 0 as singular, Polish and Russian have three forms, Japanese one),
   say so in your pull request: `trn()` needs that language's rule, and we
   will add it with you.

7. **Open a pull request** with the new `.po` file and the two
   registrations. You don't need to translate everything at once:
   untranslated strings fall back to English.

## Guidelines

- **Keep placeholders.** Text in braces (`{name}`, `{n}`, `{codec}`…) is
  replaced after translating. Keep each one exactly as it is, in whatever
  position your language needs: `"{bits}-bit/{rate} kHz {codec}"` became
  `"{codec} {bits} bits/{rate} kHz"` in Spanish.
- **Keep mnemonics.** An underscore marks the key for Alt+letter in
  buttons and menus (`"_Clear"`, `"_About Swell"`). Put one underscore
  before a letter of your translation, avoiding clashes with other items
  in the same menu or dialog.
- **Match the tone.** Short, plain and friendly, as in the English and
  Spanish versions. Use your desktop's usual terms (GNOME and KDE
  translations are a good reference) and keep TIDAL's product names
  (Hi-Res Lossless, Mix, Track Radio) the way TIDAL's own apps use them in
  your language.
- **Typography.** Use your language's quotation marks and ellipsis (`…`,
  one character) where the English text does.
- Don't translate keyboard key names that appear inside shortcuts; GTK
  does that.

## Updating after the code changes

When strings are added or changed in `src/`, regenerate the template and
merge it into each catalogue:

```bash
po/update-pot
msgmerge --update --backup=none --no-fuzzy-matching po/fr.po po/swell.pot
msgfmt --check --statistics -o /dev/null po/fr.po    # counts what is left
```

`po/update-pot --check` exits with an error when the template is out of
date. The test `every_string_is_translated` (`cargo test`) checks that the
Spanish catalogue covers every `tr()` string; other languages fall back to
English for what is missing.

## Trying it out

Swell reads the language from the environment, so you can try a catalogue
without changing your system language:

```bash
LANG=fr_FR.UTF-8 cargo run --release
```

GTK's own strings (standard dialogs, key names) only follow if that locale
is installed on your system (`locale -a`); Swell's strings follow either
way. `LANG=C.UTF-8` shows the original English.

## For developers: making strings translatable

- Wrap user-visible text in `tr("…")`, and counts in
  `trn("{n} track", "{n} tracks", n)`; replace placeholders after
  translating: `tr("by {name}").replace("{name}", owner)`.
- Strings stored in tables and translated later are marked with
  `n_("…")` (see `src/ui/shortcuts.rs`). `po/update-pot` also recognises
  the plural helpers `counted(n, "…", "…")` and `unit(n, "…", "…")`, and
  sort options (`("id", "Label", bool)`); anything else that reaches `tr()`
  through a variable must use one of these forms, or it won't reach the
  template.
- The embedded parser understands what `msgmerge` writes: `msgid`,
  `msgid_plural`, `msgstr`, `msgstr[n]`, continuation lines, comments and
  the `fuzzy` flag. `msgctxt` isn't supported, so each English string has
  one translation.
