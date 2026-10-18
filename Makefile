# Build and install Swell on Linux.
#
#   make                                 # cargo build --release --locked
#   make install PREFIX=/usr DESTDIR=pkg # stage an install for a package
#   sudo make install                    # install under /usr/local
#   make uninstall                       # remove what `install` put in place
#   make check                           # validate the desktop file and metainfo
#
# `install` never builds: run `make` first (as your user, not as root).
# Variables: CARGO, CARGOFLAGS, CARGO_TARGET_DIR, PREFIX, DESTDIR, BINDIR,
# DATADIR. The binary embeds its icons and translations, so it also runs
# from anywhere without being installed.

APP_ID     := io.github.oxakromax.SwellPlayer
BIN        := swell-player

CARGO      ?= cargo
CARGOFLAGS ?=
# Cargo's own variable: honour it when set in the environment.
CARGO_TARGET_DIR ?= target

PREFIX     ?= /usr/local
DESTDIR    ?=
BINDIR     ?= $(PREFIX)/bin
DATADIR    ?= $(PREFIX)/share
APPSDIR    ?= $(DATADIR)/applications
METAINFODIR ?= $(DATADIR)/metainfo
ICONSDIR   ?= $(DATADIR)/icons/hicolor
LICENSEDIR ?= $(DATADIR)/licenses/$(BIN)
DOCDIR     ?= $(DATADIR)/doc/$(BIN)

INSTALL    ?= install
DESKTOP_FILE_VALIDATE ?= desktop-file-validate
APPSTREAMCLI ?= appstreamcli

TARGET_BIN := $(CARGO_TARGET_DIR)/release/$(BIN)
DOCS       := $(wildcard README.md CHANGELOG.md)

.PHONY: all build test check install uninstall clean

all: build

build:
	$(CARGO) build --release --locked $(CARGOFLAGS)

test:
	$(CARGO) test --release --locked $(CARGOFLAGS)

check:
	$(DESKTOP_FILE_VALIDATE) data/$(APP_ID).desktop
	$(APPSTREAMCLI) validate --no-net data/$(APP_ID).metainfo.xml

install:
	@test -x "$(TARGET_BIN)" || { \
		echo "$(TARGET_BIN) is missing: run 'make' first (or set CARGO_TARGET_DIR)." >&2; \
		exit 1; }
	$(INSTALL) -Dm755 "$(TARGET_BIN)" "$(DESTDIR)$(BINDIR)/$(BIN)"
	$(INSTALL) -Dm644 data/$(APP_ID).desktop "$(DESTDIR)$(APPSDIR)/$(APP_ID).desktop"
	$(INSTALL) -Dm644 data/$(APP_ID).metainfo.xml "$(DESTDIR)$(METAINFODIR)/$(APP_ID).metainfo.xml"
	$(INSTALL) -Dm644 data/icons/scalable/apps/$(APP_ID).svg \
		"$(DESTDIR)$(ICONSDIR)/scalable/apps/$(APP_ID).svg"
	$(INSTALL) -Dm644 data/icons/symbolic/apps/$(APP_ID)-symbolic.svg \
		"$(DESTDIR)$(ICONSDIR)/symbolic/apps/$(APP_ID)-symbolic.svg"
	$(INSTALL) -Dm644 COPYING "$(DESTDIR)$(LICENSEDIR)/COPYING"
	$(INSTALL) -dm755 "$(DESTDIR)$(DOCDIR)"
	$(foreach doc,$(DOCS),$(INSTALL) -m644 $(doc) "$(DESTDIR)$(DOCDIR)/$(doc)";)
	@if [ -z "$(DESTDIR)" ] && [ -z "$$FLATPAK_ID" ]; then \
		command -v gtk-update-icon-cache >/dev/null && \
			gtk-update-icon-cache -qtf "$(ICONSDIR)" || true; \
		command -v update-desktop-database >/dev/null && \
			update-desktop-database -q "$(APPSDIR)" || true; \
	fi

uninstall:
	rm -f "$(DESTDIR)$(BINDIR)/$(BIN)"
	rm -f "$(DESTDIR)$(APPSDIR)/$(APP_ID).desktop"
	rm -f "$(DESTDIR)$(METAINFODIR)/$(APP_ID).metainfo.xml"
	rm -f "$(DESTDIR)$(ICONSDIR)/scalable/apps/$(APP_ID).svg"
	rm -f "$(DESTDIR)$(ICONSDIR)/symbolic/apps/$(APP_ID)-symbolic.svg"
	rm -rf "$(DESTDIR)$(LICENSEDIR)" "$(DESTDIR)$(DOCDIR)"
	@if [ -z "$(DESTDIR)" ] && [ -z "$$FLATPAK_ID" ]; then \
		command -v gtk-update-icon-cache >/dev/null && \
			gtk-update-icon-cache -qtf "$(ICONSDIR)" || true; \
		command -v update-desktop-database >/dev/null && \
			update-desktop-database -q "$(APPSDIR)" || true; \
	fi

clean:
	$(CARGO) clean
