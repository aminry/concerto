# Concerto top-level Makefile.
#
# Thin convenience wrapper around the per-platform install scripts. The
# real logic lives in scripts/install-macos.sh and scripts/uninstall-macos.sh
# (and, eventually, scripts/install-linux.sh for V1.0).

UNAME_S := $(shell uname -s)

.PHONY: install uninstall install-macos uninstall-macos help dev-embedded

help:
	@echo "Concerto install targets:"
	@echo "  make install         Install for the current platform ($(UNAME_S))"
	@echo "  make uninstall       Uninstall from the current platform"
	@echo "  make install-macos   Force the macOS LaunchAgent install"
	@echo "  make uninstall-macos Force the macOS LaunchAgent uninstall"
	@echo ""
	@echo "  make dev-embedded    Run the desktop app with Core embedded (scratch data)"
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

## Run the desktop app with Core embedded in-process, against a scratch
## data root so it never touches ~/concerto. Frontend HMR (Vite) is live;
## the desktop crate rebuilds on change. NOTE: Tauri's dev watcher watches
## the src-tauri crate — to also hot-reload edits to crates/core, run with
## cargo-watch instead (requires `cargo install cargo-watch`):
##   cd apps/desktop && CONCERTO_HOME=$$(mktemp -d) \
##     cargo watch -w ../../crates -w src -s 'pnpm tauri dev -f embedded-core'
dev-embedded:
	cd apps/desktop && CONCERTO_HOME=$${CONCERTO_HOME:-$$(mktemp -d -t concerto-dev.XXXXXX)} \
		pnpm tauri dev -f embedded-core
