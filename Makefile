# Concerto top-level Makefile.
#
# Thin convenience wrapper around the per-platform install scripts. The
# real logic lives in scripts/install-macos.sh and scripts/uninstall-macos.sh
# (and, eventually, scripts/install-linux.sh for V1.0).

UNAME_S := $(shell uname -s)

.PHONY: install uninstall install-macos uninstall-macos help dev-embedded dev-embedded-scratch smoke-embedded stop-core build-embedded

help:
	@echo "Concerto install targets:"
	@echo "  make install         Install for the current platform ($(UNAME_S))"
	@echo "  make uninstall       Uninstall from the current platform"
	@echo "  make install-macos   Force the macOS LaunchAgent install"
	@echo "  make uninstall-macos Force the macOS LaunchAgent uninstall"
	@echo ""
	@echo "  make dev-embedded         Run desktop + embedded Core (real ~/concerto data)"
	@echo "  make dev-embedded-scratch Same, but against an isolated scratch data root"
	@echo "  make stop-core            Stop the standalone Core daemon (macOS)"
	@echo "  make smoke-embedded       Headless smoke gate for embedded-core mode"
	@echo "  make build-embedded       Build the standalone 'Concerto Embedded' app bundle"
	@echo ""
	@echo "Linux systemd / Windows Service Manager support lands in V1.0."

install:
ifeq ($(UNAME_S),Darwin)
	@$(MAKE) install-macos
else
	@echo "make install: unsupported platform ($(UNAME_S)); Linux/Windows land in V1.0" >&2
	@exit 1
endif

uninstall:
ifeq ($(UNAME_S),Darwin)
	@$(MAKE) uninstall-macos
else
	@echo "make uninstall: unsupported platform ($(UNAME_S)); Linux/Windows land in V1.0" >&2
	@exit 1
endif

install-macos:
	@./scripts/install-macos.sh

uninstall-macos:
	@./scripts/uninstall-macos.sh

## Run the desktop app with Core embedded in-process against your REAL
## ~/concerto data (the same folder the standalone daemon uses). Hot-reloads
## the frontend (Vite HMR), the src-tauri crate, and crates/core (via
## cargo watch). Run `make stop-core` first if the daemon is running.
## Requires cargo-watch: `cargo install cargo-watch`.
dev-embedded:
	@./scripts/dev-embedded.sh

## Same hot-reload loop, but against an isolated scratch data root so it
## never touches ~/concerto. Use for throwaway/isolated testing.
dev-embedded-scratch:
	@CONCERTO_HOME="$${CONCERTO_HOME:-$$(mktemp -d -t concerto-dev.XXXXXX)}" ./scripts/dev-embedded.sh

## Headless smoke gate for embedded-core mode: builds the desktop binary
## with the feature and runs the in-process boot tests (no GUI). The main
## CI smoke gate (scripts/smoke.sh) deliberately avoids the Tauri toolchain;
## run this locally to exercise the one-process path.
smoke-embedded:
	@./scripts/smoke-embedded.sh

## Stop the standalone Core daemon (launchd) and release its PID lock so
## embedded-real mode can boot in-process. macOS-only.
stop-core:
	@./scripts/stop-core.sh

## Build a self-contained "Concerto Embedded" .app/.dmg (Desktop + Core in
## one binary) for local distribution. Unsigned. The config overlay gives it
## a distinct product name + identifier so it installs alongside the normal app.
build-embedded:
	cd apps/desktop && pnpm tauri build -f embedded-core \
		--config src-tauri/tauri.embedded.conf.json
