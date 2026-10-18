# Security policy

## Supported versions

Security fixes go into the latest release. Please check that a problem
still happens with it (or with the `master` branch) before reporting it.

| Version | Supported |
|---|---|
| 1.x (latest) | Yes |
| Older | No |

## Reporting a vulnerability

**Please don't open a public issue for security problems.** Report them
privately through GitHub's
[private vulnerability reporting](https://github.com/oxakromax/swell-player/security/advisories/new)
(the repository's **Security** tab → **Report a vulnerability**).

Include what you found, how to reproduce it, the Swell version and
platform, and what an attacker could do with it. You should get an answer
within a week; once a fix is released, the advisory is published and you
are credited unless you prefer otherwise.

Problems in TIDAL's own services are out of scope: report those to TIDAL.

## How Swell handles your credentials

- Swell never sees your TIDAL password: you sign in on TIDAL's own login
  page (or approve a device code at `link.tidal.com`), and Swell receives
  OAuth tokens.
- The tokens are stored in the system keyring (on Linux, the Secret
  Service: GNOME Keyring, KWallet, KeePassXC…, or the secret portal in
  Flatpak), never in plain-text files. Signing out deletes them, together
  with the cache, the saved queue and the search history.
- All traffic goes to TIDAL's servers over HTTPS (rustls). Swell has no
  telemetry or third-party services.
- Hi-Res stream manifests are written while playing to a private,
  per-process directory under `$XDG_RUNTIME_DIR`, and removed afterwards.

## Sharing logs safely

Debug logs (`RUST_LOG=swell_player=debug`) can contain stream URLs, which
are signed and grant temporary access to a track. They expire quickly, but
please look through logs before attaching them to a public issue, and never
paste tokens, the "Oops" page address from the browser sign-in, or the
contents of your keyring.
