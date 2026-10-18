<!-- Thanks for contributing! Please read CONTRIBUTING.md first. -->

## What and why

<!-- What does this change, and why? Link the issue it fixes, if any ("Fixes #123"). -->

## How it was tested

<!-- Steps you followed, platforms you tried. For interface changes, add screenshots (light and dark, wide and narrow if the layout changes). -->

## Checklist

- [ ] `cargo fmt --all`, `cargo clippy --release --all-targets` (no new warnings) and `cargo test --release` pass
- [ ] New or changed UI strings use `tr()`/`trn()`, `po/update-pot` was run, and `po/es.po` has their translations (or I've asked for help)
- [ ] Nothing blocks the GTK main thread (network, disk, keyring and decoding go through the runtime)
- [ ] New icon-only buttons have a tooltip and an accessible label
- [ ] User-visible changes are noted in `CHANGELOG.md` under **Unreleased**
