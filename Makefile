CARGO ?= cargo
INSTALL ?= install
CARGO_TARGET_DIR ?= target
PREFIX ?= /usr
DESTDIR ?=
BINDIR ?= $(PREFIX)/bin
DATADIR ?= $(PREFIX)/share
ICON_SOURCE ?= assets/wwt.svg

BINARY := $(CARGO_TARGET_DIR)/release/wwt
DESKTOP_FILE := assets/wwt.desktop

export CARGO_TARGET_DIR

.PHONY: all build install uninstall test-install

all: build

build:
	$(CARGO) build --locked --release -p wwt

install:
	@test -x "$(BINARY)" || { \
		printf '%s\n' 'wwt is not built; run make first' >&2; \
		exit 1; \
	}
	$(INSTALL) -Dm755 "$(BINARY)" "$(DESTDIR)$(BINDIR)/wwt"
	$(INSTALL) -Dm644 "$(DESKTOP_FILE)" "$(DESTDIR)$(DATADIR)/applications/wwt.desktop"
	@if test -f "$(ICON_SOURCE)"; then \
		$(INSTALL) -Dm644 "$(ICON_SOURCE)" \
			"$(DESTDIR)$(DATADIR)/icons/hicolor/scalable/apps/wwt.svg"; \
	fi

uninstall:
	rm -f -- "$(DESTDIR)$(BINDIR)/wwt"
	rm -f -- "$(DESTDIR)$(DATADIR)/applications/wwt.desktop"
	rm -f -- "$(DESTDIR)$(DATADIR)/icons/hicolor/scalable/apps/wwt.svg"

test-install:
	sh tests/install.sh
