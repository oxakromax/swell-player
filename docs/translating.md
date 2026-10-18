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
| `po/<lang>.po` | One catalogue per language: `es`, `fr`, `de`, `it`, `pt_BR`, `ru`, `ja`, `ko`, `zh_CN`, `zh_TW`. |
| `po/data/<lang>.json` | The translations of the desktop file and the AppStream metainfo (see [Desktop file and metainfo](#desktop-file-and-metainfo)). |
| `po/apply-data-translations` | Writes those into `data/` (Python 3, no dependencies). |
| `README.<lang>.md` | The README in each language, linked from the language bar at the top of [`README.md`](../README.md). |

Any `.po` editor works: [Poedit](https://poedit.net),
[GNOME Translation Editor](https://wiki.gnome.org/Apps/Gtranslator),
[Lokalize](https://apps.kde.org/lokalize/), or a plain text editor.

## Improving an existing translation

Edit `po/<language>.po`, rebuild, and check the result (see
[Trying it out](#trying-it-out)). Entries marked `#, fuzzy` are ignored by
Swell: review them and remove the flag once they are right.

## Adding a language

The steps below use Polish (`pl`) as the example; use your language's
ISO 639 code, with the region only when the catalogue is for one variant
(`pt_BR`, `zh_TW`).

1. **Create the catalogue** from the template:

   ```bash
   msginit -i po/swell.pot -l pl -o po/pl.po --no-translator
   ```

   (or `cp po/swell.pot po/pl.po`). In the header at the top, check
   `Language: pl` and `Plural-Forms` (msginit fills in the standard one;
   with a plain copy, set it yourself and give each plural entry as many
   `msgstr[N]` lines as `nplurals` says), fill in `Last-Translator` and
   `Language-Team` if you like, and remove the `#, fuzzy` line above the
   header.

2. **Translate** every `msgstr`. See [Guidelines](#guidelines) below.

3. **Register the catalogue** in `CATALOGUES` in
   [`src/i18n.rs`](../src/i18n.rs), which embeds it in the binary and adds
   it to the Language preference, under the language's own name
   ("Polski", not "Polish"):

   ```rust
   Language {
       code: "pl",
       name: "Polski",
       source: include_str!("../po/pl.po"),
   },
   ```

4. **Plural rule.** Swell does not evaluate the `Plural-Forms` expression:
   each language's rule is written in Rust in `plural_form()` in
   `src/i18n.rs`, with its number of forms in `plural_count()`. Languages
   with English's rule (`n != 1`: Spanish, German, Italian…) need nothing;
   otherwise add a case for your code to both, and to the `plural_rules`
   test. The tests check that `plural_count()` matches the header's
   `nplurals`, that every form is reachable, and that every plural entry
   has that many forms.

5. **Locale detection and TIDAL's locale.** `supported()` in
   `src/i18n.rs` maps system locales (`pl_PL.UTF-8`, `pl-PL`) to the
   catalogue by language code; it needs a case only for special mappings
   (`pt*` → `pt_BR`, Chinese by region and script). `tidal_locale_for()`
   asks TIDAL for the user's regional variant of a two-letter language
   (`fr_CA`) or, failing that, the language's usual locale: add yours to
   its list (`"pl" => "pl_PL"`).

6. **Desktop file and metainfo.** Add `po/data/pl.json` and run
   `po/apply-data-translations` (see
   [Desktop file and metainfo](#desktop-file-and-metainfo)).

7. **README.** Translate [`README.md`](../README.md) into `README.pl.md`
   (keep code blocks, commands, links and image paths unchanged) and add
   the language to the language bar at the top of every `README*.md`.

8. **Open a pull request.** You don't need to translate everything at
   once: untranslated strings fall back to English. A catalogue that is
   not complete yet carries `"X-Swell-Partial: yes\n"` in its header,
   which excuses it from the completeness test (see
   [Tests](#tests)); remove that line once every string is translated.

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
date.

### Tests

`cargo test` checks the catalogues (`src/i18n.rs`):

- `every_string_is_translated`: every catalogue covers every `tr()`
  string and every plural form, **except** catalogues whose header has
  `"X-Swell-Partial: yes\n"`. That marker is for catalogues that are not
  complete yet, such as the header-only placeholders a language starts
  with; a complete catalogue must not have it, so the test is strict for
  it from then on.
- `catalogues_are_consistent`, for every catalogue, partial or not: the
  header's `Language` is the catalogue's code and its `nplurals` is the
  number of forms `plural_form()` uses; every plural entry has that many
  `msgstr[N]`; and every translation keeps the `{placeholders}` of its
  msgid. A plural form must have exactly the placeholders of the
  `msgid_plural`, except a form used only for n = 1 (form 0 with the
  `n != 1` rule), which may leave the number out ("Se saltó una
  canción"). With `n > 1` (French, Portuguese) form 0 also covers 0, and
  in Russian form 0 also covers 21, 31…, so those forms must keep `{n}`.

## Desktop file and metainfo

The texts of
[`data/io.github.oxakromax.SwellPlayer.desktop`](../data/io.github.oxakromax.SwellPlayer.desktop)
and
[`data/io.github.oxakromax.SwellPlayer.metainfo.xml`](../data/io.github.oxakromax.SwellPlayer.metainfo.xml)
(the app's description in software centres) are translated in
`po/data/<lang>.json`, named with the catalogue's code. Each file is one
flat JSON object, in UTF-8, mapping an English text to its translation:

```json
{
  "Music Player": "Reproductor de música",
  "Listen to TIDAL in a native app": "Escucha TIDAL en una app nativa",
  "music;tidal;streaming;hi-res;lossless;flac;player;audio;": "música;tidal;streaming;hi-res;sin pérdida;flac;reproductor;audio;",
  "An album page": "La página de un álbum"
}
```

- **Keys** are the English texts exactly as they are in those files, each
  on one line: the desktop file's `GenericName`, `Comment` and `Keywords`
  (the whole `;`-separated list is one text; the metainfo's `summary` is
  the same text as `Comment`), and the metainfo's description paragraphs
  (`<p>`; the XML's line breaks and indentation become single spaces) and
  list items (`<li>`), screenshot captions and the releases' description
  paragraphs and items. [`po/data/es.json`](../po/data/es.json) has them
  all.
- **Values** are plain text: no XML markup or escapes (`&` is written
  `&`; the script escapes it). A missing or empty value leaves that text
  in English for the language.
- **Keywords**: translate each word of the list, keep the `;` separators
  (and the brand words `tidal`, `flac`, `hi-res` if your language uses
  them as is). The metainfo's translated `<keyword>`s are the words of
  that list that are not English keywords; they are not listed
  separately.

Then regenerate the two files:

```bash
po/apply-data-translations          # rewrites every Comment[xx]= and xml:lang="xx" entry
po/apply-data-translations --check  # fails if they are not what po/data gives
appstreamcli validate --no-net data/io.github.oxakromax.SwellPlayer.metainfo.xml
desktop-file-validate data/io.github.oxakromax.SwellPlayer.desktop
```

Don't edit the translated entries in `data/` by hand: the script
regenerates all of them, the English text first and then one entry per
language, in file-name order. It warns about English texts a language
doesn't translate and about JSON entries that match no English text (the
English changed: update the key and the translation).
`po/apply-data-translations --extract <lang>` prints the translations
already in the files as JSON, to start a `po/data` file from them.

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
